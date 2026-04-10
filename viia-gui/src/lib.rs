use base64::{Engine as _, engine::general_purpose};
use image::DynamicImage;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use viia_core::{
    Animation, InternalCommand, PlaybackState, RuntimeAction, SlideshowManager, TimingCommand,
    collect_image_paths, parse_slideshow_spec, update_prefetch,
};

#[derive(Clone, serde::Serialize)]
struct FramePayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    data_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blob_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    zoom_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_path: Option<String>,
}

struct AppState {
    tx: mpsc::Sender<String>,
}

struct FrameState {
    buffer: Mutex<Vec<u8>>,
    content_type: Mutex<String>,
}

#[tauri::command]
async fn send_command(cmd: String, state: State<'_, AppState>) -> Result<(), String> {
    info!("send_command called from frontend with: {}", cmd);
    state.tx.send(cmd).await.map_err(|e| {
        error!("Failed to send command to channel: {}", e);
        e.to_string()
    })
}

#[tauri::command]
async fn start_engine(state: State<'_, AppState>) -> Result<(), String> {
    info!("start_engine called from frontend");
    // Signal the engine to force a redraw now that the frontend is ready to receive events.
    // The engine loop is waiting for a "start" command to unblock.
    state.tx.send("start".to_string()).await.map_err(|e| {
        error!("Failed to send 'start' command to channel: {}", e);
        e.to_string()
    })
}

#[tauri::command]
fn log_to_terminal(msg: String) {
    info!("Frontend: {}", msg);
}

fn engine_loop(
    app: AppHandle,
    paths: Vec<PathBuf>,
    mut rx: mpsc::Receiver<String>,
    frame_state: Arc<FrameState>,
    prefetch: usize,
) {
    info!("engine_loop started in background thread");

    // Skip waiting, just start the engine directly and handle commands as they come
    info!(
        "Frontend is ready. Starting GUI engine loop with {} input paths",
        paths.len()
    );
    let resolved_paths = collect_image_paths(paths);
    info!("Resolved {} image paths", resolved_paths.len());

    let mut animations = Vec::new();
    for path in resolved_paths {
        if let Ok(anim) = Animation::skim(path.clone()) {
            animations.push(anim);
        } else {
            warn!("Failed to skim animation for path: {:?}", path);
        }
    }

    if animations.is_empty() {
        warn!("No valid animations found, emitting empty status");
        let _ = app.emit("status", "No images provided.");
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.set_title("viia - No images provided.");
        }
        return;
    }

    info!("Successfully loaded {} animations", animations.len());

    let mut current_idx = 0;
    update_prefetch(&mut animations, current_idx, prefetch);
    let default_cmd = vec![TimingCommand {
        loops: None,
        time_secs: None,
        infinite: true,
    }];
    let mut manager = SlideshowManager::new(default_cmd.clone());
    manager.load_animation(&animations[current_idx]);

    let mut zoom_mode = viia_core::ZoomMode::Shrink;

    let mut last_tick = Instant::now();
    let mut last_rendered_frame = usize::MAX;
    let mut last_rendered_idx = usize::MAX;

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_tick);
        last_tick = now;

        update_prefetch(&mut animations, current_idx, prefetch);

        // Process commands
        let mut commands_to_process = Vec::new();
        while let Ok(line) = rx.try_recv() {
            commands_to_process.push(line);
        }

        let mut filtered_commands = Vec::new();
        let mut cancel_nav = false;
        for line in commands_to_process.into_iter().rev() {
            let line_trim = line.trim();
            if line_trim == "__cancel_pending__" {
                cancel_nav = true;
                continue;
            }
            if cancel_nav && (line_trim == "r" || line_trim == "l") {
                debug!("Cancelled pending navigation command: {}", line_trim);
                continue;
            }
            filtered_commands.push(line);
        }
        filtered_commands.reverse();

        for line in filtered_commands {
            let line = line.trim();
            if !line.is_empty() {
                debug!("Processing internal command: '{}'", line);
                if line == "start" {
                    info!("Received start command, but engine loop already running");
                    // Force a redraw when we receive "start" late
                    last_rendered_frame = usize::MAX;
                    continue;
                }
                match InternalCommand::parse_line(line) {
                    Ok(cmd) => match cmd.action {
                        RuntimeAction::Quit => {
                            info!("Quit command received, exiting engine loop");
                            app.exit(0);
                            return;
                        }
                        RuntimeAction::TogglePause => {
                            manager.toggle_pause();
                            last_rendered_frame = usize::MAX;
                            info!("Toggled pause state, now: {:?}", manager.state());
                        }
                        RuntimeAction::ShowNext => {
                            current_idx = (current_idx + 1) % animations.len();
                            info!(
                                "Showing next animation: index {} ({})",
                                current_idx,
                                animations[current_idx].source_path.display()
                            );
                            update_prefetch(&mut animations, current_idx, prefetch);
                            manager.load_animation(&animations[current_idx]);
                            last_rendered_frame = usize::MAX;
                        }
                        RuntimeAction::ShowPrevious => {
                            if current_idx == 0 {
                                current_idx = animations.len() - 1;
                            } else {
                                current_idx -= 1;
                            }
                            info!(
                                "Showing previous animation: index {} ({})",
                                current_idx,
                                animations[current_idx].source_path.display()
                            );
                            update_prefetch(&mut animations, current_idx, prefetch);
                            manager.load_animation(&animations[current_idx]);
                            last_rendered_frame = usize::MAX;
                        }
                        RuntimeAction::ShowCurrent => {
                            debug!("ShowCurrent command forcing redraw");
                            last_rendered_frame = usize::MAX;
                        }
                        RuntimeAction::Dimension { .. } => {
                            debug!(
                                "Dimension command ignored in GUI (handled by Tauri window resizing)"
                            );
                        }
                        RuntimeAction::Zoom { mode } => {
                            zoom_mode = mode;
                            last_rendered_frame = usize::MAX;
                            debug!("Zoom mode updated in GUI to {:?}", zoom_mode);
                        }
                        RuntimeAction::StartSlideshow { cmd } => {
                            let spec = cmd.join(" ");
                            info!("Starting slideshow with spec: '{}'", spec);
                            let parent_dir = animations[current_idx]
                                .source_path
                                .parent()
                                .unwrap_or(std::path::Path::new(""));
                            if let Ok(cmds) = parse_slideshow_spec(&spec, parent_dir)
                                && !cmds.is_empty()
                            {
                                info!("Parsed {} timing commands for slideshow", cmds.len());
                                manager.set_commands(cmds, &animations[current_idx]);
                                last_rendered_frame = usize::MAX;
                            } else {
                                warn!(
                                    "Failed to parse slideshow spec or spec resulted in empty commands"
                                );
                            }
                        }
                        RuntimeAction::Help => {
                            unreachable!("Help is handled internally by parse_line")
                        }
                    },
                    Err(e) => {
                        if e.kind() == clap::error::ErrorKind::DisplayHelp {
                            tracing::info!("{}", e);
                            let version_info = format!(
                                "\nviia {} (git: {}, build: {})",
                                env!("CARGO_PKG_VERSION"),
                                env!("GIT_HASH"),
                                env!("BUILD_TIMESTAMP")
                            );
                            let help_text = format!("{}{}", e, version_info);
                            let _ = app.emit("help", help_text);
                        } else {
                            error!("Invalid command: {}", e);
                        }
                    }
                }
            }
        }

        let should_advance = manager.tick(dt, &animations[current_idx]);
        if should_advance {
            current_idx = (current_idx + 1) % animations.len();
            info!(
                "Automatically advancing to next animation: index {} ({})",
                current_idx,
                animations[current_idx].source_path.display()
            );
            update_prefetch(&mut animations, current_idx, prefetch);
            manager.load_animation(&animations[current_idx]);
            last_rendered_frame = usize::MAX;
        }

        let frame_idx = manager.current_frame_index();
        let needs_render = current_idx != last_rendered_idx || frame_idx != last_rendered_frame;

        if needs_render {
            debug!(
                "Needs render triggered: current_idx={}, last_idx={}, frame_idx={}, last_frame={}",
                current_idx, last_rendered_idx, frame_idx, last_rendered_frame
            );

            let zoom_str = match &zoom_mode {
                viia_core::ZoomMode::Fit => "fit".to_string(),
                viia_core::ZoomMode::Shrink => "shrink".to_string(),
                viia_core::ZoomMode::Fixed(s) => s.to_string(),
            };

            if let viia_core::AnimationState::Parsed(frames) = &animations[current_idx].state
                && let Some(frame) = frames.get(frame_idx)
            {
                if frames.len() == 1 {
                    let path = animations[current_idx]
                        .source_path
                        .to_string_lossy()
                        .to_string();
                    debug!("Emitting path {} for animation {}", path, current_idx);
                    if let Err(e) = app.emit(
                        "frame",
                        FramePayload {
                            data_uri: None,
                            path: Some(path.clone()),
                            blob_url: None,
                            zoom_mode: Some(zoom_str),
                            image_path: Some(path),
                        },
                    ) {
                        error!("Failed to emit frame event: {}", e);
                    }
                } else {
                    // Convert frame.data (RgbaImage) to bytes for the custom protocol
                    let dyn_img = DynamicImage::ImageRgba8(frame.data.clone());
                    let mut cursor = Cursor::new(Vec::new());

                    if dyn_img
                        .write_to(&mut cursor, image::ImageFormat::Png)
                        .is_ok()
                    {
                        let buffer = cursor.into_inner();
                        *frame_state.buffer.lock().unwrap() = buffer;
                        *frame_state.content_type.lock().unwrap() = "image/png".to_string();

                        // Encode parameters for caching
                        let path_str = animations[current_idx]
                            .source_path
                            .to_string_lossy()
                            .to_string();
                        let path_b64 = general_purpose::URL_SAFE_NO_PAD.encode(path_str.as_bytes());

                        #[cfg(any(windows, target_os = "android"))]
                        let protocol_prefix = "http://viia.localhost";
                        #[cfg(not(any(windows, target_os = "android")))]
                        let protocol_prefix = "viia://localhost";

                        let blob_url = if let viia_core::ZoomMode::Fixed(_) = zoom_mode {
                            format!(
                                "{}/frame?path={}&frame={}&scale={}",
                                protocol_prefix, path_b64, frame_idx, zoom_str
                            )
                        } else {
                            format!(
                                "{}/frame?path={}&frame={}",
                                protocol_prefix, path_b64, frame_idx
                            )
                        };

                        debug!(
                            "Emitting blob url {} for animation {}",
                            blob_url, current_idx
                        );
                        if let Err(e) = app.emit(
                            "frame",
                            FramePayload {
                                data_uri: None,
                                path: None,
                                blob_url: Some(blob_url),
                                zoom_mode: Some(zoom_str),
                                image_path: Some(path_str),
                            },
                        ) {
                            error!("Failed to emit frame event: {}", e);
                        }
                    } else {
                        error!("Failed to encode frame to PNG");
                    }
                }
            } else if let viia_core::AnimationState::Error(err) = &animations[current_idx].state {
                error!("Failed to render image: {}", err);
                
                // Clear the image and we will emit an error in the status message
                let path = animations[current_idx]
                    .source_path
                    .to_string_lossy()
                    .to_string();
                if let Err(e) = app.emit(
                    "frame",
                    FramePayload {
                        data_uri: Some("data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7".to_string()), // 1x1 transparent gif
                        path: None,
                        blob_url: None,
                        zoom_mode: Some(zoom_str),
                        image_path: Some(path),
                    },
                ) {
                    error!("Failed to emit frame event: {}", e);
                }
            } else {
                warn!(
                    "Animation state not parsed or frame not found! idx={}, frame_idx={}",
                    current_idx, frame_idx
                );
            }

            let file_name = animations[current_idx]
                .source_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();

            let mut status_msg = format!(
                "[{}/{}] {} | {}",
                current_idx + 1,
                animations.len(),
                file_name,
                if manager.state() == PlaybackState::Playing {
                    "Playing"
                } else {
                    "Paused"
                }
            );

            if let viia_core::AnimationState::Error(err) = &animations[current_idx].state {
                status_msg = format!(
                    "[{}/{}] {} (Error: {}) | {}",
                    current_idx + 1,
                    animations.len(),
                    file_name,
                    err,
                    if manager.state() == PlaybackState::Playing {
                        "Playing"
                    } else {
                        "Paused"
                    }
                );
            }

            debug!("Emitting status update: {}", status_msg);
            if let Err(e) = app.emit("status", status_msg) {
                error!("Failed to emit status event: {}", e);
            }

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("viia - {}", file_name));
            }

            last_rendered_idx = current_idx;
            last_rendered_frame = frame_idx;
        }

        let sleep_dur = manager.time_until_next_frame(&animations[current_idx]);
        let poll_dur = sleep_dur.min(Duration::from_millis(16)); // ~60fps poll
        std::thread::sleep(poll_dur);
    }
}

pub fn run_gui(
    paths: Vec<PathBuf>,
    dimension: Option<String>,
    prefetch: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        "Initializing Tauri GUI application with {} input paths",
        paths.len()
    );
    let (tx, rx) = mpsc::channel(32);
    // Clone tx so we can pass it to state and keep a copy if needed, but AppState takes ownership
    let tx_state = tx.clone();

    let frame_state = Arc::new(FrameState {
        buffer: Mutex::new(Vec::new()),
        content_type: Mutex::new("image/png".to_string()),
    });
    let frame_state_protocol = frame_state.clone();

    tauri::Builder::default()
        .register_uri_scheme_protocol("viia", move |_app, _request| {
            let buffer = frame_state_protocol.buffer.lock().unwrap().clone();
            let content_type = frame_state_protocol.content_type.lock().unwrap().clone();

            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", content_type)
                .header("Access-Control-Allow-Origin", "*")
                .header("Cache-Control", "public, max-age=31536000")
                .body(buffer)
                .unwrap_or_else(|_| {
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Vec::new())
                        .unwrap()
                })
        })
        .manage(AppState { tx: tx_state })
        .invoke_handler(tauri::generate_handler![
            send_command,
            start_engine,
            log_to_terminal
        ])
        .setup(move |app| {
            info!("Tauri setup hook called, spawning engine loop thread");
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                engine_loop(handle, paths, rx, frame_state, prefetch);
            });

            if let Some(window) = app.get_webview_window("main") {
                let mut target_width = None;
                let mut target_height = None;

                if let Some(dim) = dimension {
                    let parts: Vec<&str> = dim.split('x').collect();
                    if parts.len() == 2
                        && let (Ok(w), Ok(h)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>())
                    {
                        target_width = Some(w);
                        target_height = Some(h);
                    }
                }

                if (target_width.is_none() || target_height.is_none())
                    && let Ok(Some(monitor)) = window.current_monitor()
                {
                    let size = monitor.size();
                    let scale_factor = monitor.scale_factor();

                    // Using LogicalSize to account for screen scale ratio
                    let logical_w = (size.width as f64 / scale_factor) * 2.0 / 3.0;
                    let logical_h = (size.height as f64 / scale_factor) * 2.0 / 3.0;

                    target_width = Some(logical_w);
                    target_height = Some(logical_h);
                }

                if let (Some(w), Some(h)) = (target_width, target_height) {
                    let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
                        width: w,
                        height: h,
                    }));
                    let _ = window.center();
                }
            }

            // Send the start signal immediately for now, to see if the frontend was blocking on backend
            // Or maybe the frontend JS was just not running properly. Let's trigger it directly to bypass the freeze issue.
            let tx_clone = tx.clone();
            tauri::async_runtime::spawn(async move {
                // Wait 1 second and then forcibly start the engine
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                info!("Sending auto-start signal to unblock engine loop");
                let _ = tx_clone.send("start".to_string()).await;
                // Wait a bit more and force a redraw command
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let _ = tx_clone.send("e".to_string()).await;
            });

            // Open devtools automatically to see frontend console logs
            #[cfg(debug_assertions)]
            {
                use tauri::Manager;
                // Wait, tauri 2.0 creates a window called "main" by default, let's try getting it
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                } else {
                    warn!("Could not find 'main' window to open devtools");
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    info!("Tauri GUI application exited gracefully");
    Ok(())
}
