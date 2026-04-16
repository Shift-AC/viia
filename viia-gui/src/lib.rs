use base64::{Engine as _, engine::general_purpose};
use regex::Regex;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
async fn open_file_dialog(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    info!("open_file_dialog called from frontend");
    let tx = state.tx.clone();
    app.dialog().file().pick_files(move |file_paths| {
        if let Some(paths) = file_paths {
            let mut targets = Vec::new();
            for path in paths {
                if let Ok(p) = path.into_path() {
                    let safe_path = p.display().to_string().replace("\"", "\\\"");
                    targets.push(format!("\"{}\"", safe_path));
                }
            }
            if !targets.is_empty() {
                let cmd = format!("o {}", targets.join(" "));
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = tx.send(cmd).await {
                        error!("Failed to send open command: {}", e);
                    }
                });
            }
        }
    });
    Ok(())
}
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use viia_core::{
    Animation, InternalCommand, MediaUrl, PlaybackState, RuntimeAction, SlideshowManager,
    TimingCommand, parse_slideshow_spec, resolve_media_urls, shell_index_to_zero_based,
    update_prefetch, zero_based_to_shell_index,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
}

#[derive(Clone, serde::Serialize)]
struct StatusPayload {
    current_index: usize,
    total: usize,
    display_text: String,
    playback_state: String,
}

#[derive(Clone, serde::Serialize)]
struct MatchResultRow {
    index: usize,
    name: String,
}

#[derive(Clone, serde::Serialize)]
struct MatchResultsPayload {
    pattern: String,
    rows: Vec<MatchResultRow>,
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

fn display_source_name(source: &MediaUrl) -> String {
    source
        .file_name()
        .unwrap_or_else(|| source.as_str().to_string())
}

fn match_file_names(
    animations: &[Animation],
    pattern: &str,
) -> Result<Vec<(usize, String)>, regex::Error> {
    let regex = Regex::new(pattern)?;
    Ok(animations
        .iter()
        .enumerate()
        .map(|(idx, animation)| {
            (
                zero_based_to_shell_index(idx),
                display_source_name(&animation.source),
            )
        })
        .filter(|(_, name)| regex.is_match(name))
        .collect())
}

fn engine_loop(
    app: AppHandle,
    inputs: Vec<String>,
    mut rx: mpsc::Receiver<String>,
    frame_state: Arc<FrameState>,
    prefetch: usize,
) {
    info!("engine_loop started in background thread");

    // Skip waiting, just start the engine directly and handle commands as they come
    info!(
        "Frontend is ready. Starting GUI engine loop with {} input paths",
        inputs.len()
    );
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let urls = match inputs
        .iter()
        .map(|input| MediaUrl::from_input(input, &cwd))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(urls) => urls,
        Err(err) => {
            error!("Failed to normalize GUI inputs: {}", err);
            let _ = app.emit("status", format!("Invalid input: {}", err));
            return;
        }
    };
    let (resolved_urls, start_idx) = match resolve_media_urls(urls) {
        Ok(result) => result,
        Err(err) => {
            error!("Failed to resolve GUI inputs: {}", err);
            let _ = app.emit("status", format!("Failed to resolve inputs: {}", err));
            return;
        }
    };
    info!("Resolved {} image paths", resolved_urls.len());

    let mut animations = Vec::new();
    for url in resolved_urls {
        if let Ok(anim) = Animation::skim(url) {
            animations.push(anim);
        } else {
            warn!("Failed to skim animation for resolved input");
        }
    }

    let default_cmd = vec![TimingCommand {
        loops: None,
        time_secs: None,
        infinite: true,
    }];
    let mut manager = SlideshowManager::new(default_cmd.clone());

    let mut current_idx = 0;
    if !animations.is_empty() {
        info!("Successfully loaded {} animations", animations.len());
        current_idx = start_idx.min(animations.len().saturating_sub(1));
        update_prefetch(&mut animations, current_idx, prefetch);
        animations[current_idx].ensure_parsed();

        if let Err(e) = manager.load_animation(&animations[current_idx]) {
            animations[current_idx].state = viia_core::AnimationState::Error(e);
        }
    } else {
        info!("Started with empty animation list");
        let _ = app.emit("status", "No images provided.");
        let _ = app.emit("status-meta", StatusPayload {
            current_index: 0,
            total: 0,
            display_text: "No images provided.".to_string(),
            playback_state: "Paused".to_string(),
        });
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.set_title("viia - No images provided.");
        }
    }

    let mut zoom_mode = viia_core::ZoomMode::Shrink;

    let mut last_tick = Instant::now();
    let mut last_rendered_frame = usize::MAX;
    let mut last_rendered_idx = usize::MAX;
    let mut last_rendered_had_frame = false;

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_tick);
        last_tick = now;

        if !animations.is_empty() {
            update_prefetch(&mut animations, current_idx, prefetch);
        }

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
                            if animations.is_empty() { continue; }
                            current_idx = (current_idx + 1) % animations.len();
                            info!(
                                "Showing next animation: index {} ({})",
                                current_idx,
                                animations[current_idx].source.as_str()
                            );
                            update_prefetch(&mut animations, current_idx, prefetch);
                            if let Err(e) = manager.load_animation(&animations[current_idx]) {
                                animations[current_idx].state = viia_core::AnimationState::Error(e);
                            }
                            last_rendered_frame = usize::MAX;
                        }
                        RuntimeAction::ShowPrevious => {
                            if animations.is_empty() { continue; }
                            if current_idx == 0 {
                                current_idx = animations.len() - 1;
                            } else {
                                current_idx -= 1;
                            }
                            info!(
                                "Showing previous animation: index {} ({})",
                                current_idx,
                                animations[current_idx].source.as_str()
                            );
                            update_prefetch(&mut animations, current_idx, prefetch);
                            if let Err(e) = manager.load_animation(&animations[current_idx]) {
                                animations[current_idx].state = viia_core::AnimationState::Error(e);
                            }
                            last_rendered_frame = usize::MAX;
                        }
                        RuntimeAction::Goto { index } => {
                            if animations.is_empty() { continue; }
                            if let Some(target_idx) = index {
                                let zero_based_idx = match shell_index_to_zero_based(target_idx) {
                                    Ok(index) => index,
                                    Err(err) => {
                                        error!("{}", err);
                                        continue;
                                    }
                                };
                                if zero_based_idx >= animations.len() {
                                    error!(
                                        "File index {} is out of range for file list of length {}",
                                        target_idx,
                                        animations.len()
                                    );
                                    continue;
                                }
                                current_idx = zero_based_idx;
                                update_prefetch(&mut animations, current_idx, prefetch);
                                if let Err(e) = manager.load_animation(&animations[current_idx]) {
                                    animations[current_idx].state =
                                        viia_core::AnimationState::Error(e);
                                }
                            }
                            debug!("Goto command forcing redraw at index {}", current_idx);
                            last_rendered_frame = usize::MAX;
                        }
                        RuntimeAction::Match { pattern } => {
                            match match_file_names(&animations, &pattern) {
                                Ok(rows) => {
                                    info!("Matching current file list with regex: {}", pattern);
                                    let payload = MatchResultsPayload {
                                        pattern: pattern.clone(),
                                        rows: rows
                                            .into_iter()
                                            .map(|(index, name)| MatchResultRow { index, name })
                                            .collect(),
                                    };
                                    if let Err(e) = app.emit("match-results", payload) {
                                        error!("Failed to emit match-results event: {}", e);
                                    }
                                }
                                Err(err) => {
                                    error!("Invalid regex '{}': {}", pattern, err);
                                }
                            }
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
                            if animations.is_empty() { continue; }
                            let spec = cmd.join(" ");
                            info!("Starting slideshow with spec: '{}'", spec);
                            let parent_dir_path = animations[current_idx]
                                .source
                                .to_file_path()
                                .and_then(|p| p.parent().map(|x| x.to_path_buf()));
                            let parent_dir = parent_dir_path
                                .as_deref()
                                .unwrap_or(std::path::Path::new(""));
                            if let Ok(cmds) = parse_slideshow_spec(&spec, parent_dir)
                                && !cmds.is_empty()
                            {
                                info!("Parsed {} timing commands for slideshow", cmds.len());
                                if let Err(e) = manager.set_commands(cmds, &animations[current_idx])
                                {
                                    animations[current_idx].state =
                                        viia_core::AnimationState::Error(e);
                                }
                                last_rendered_frame = usize::MAX;
                            } else {
                                warn!(
                                    "Failed to parse slideshow spec or spec resulted in empty commands"
                                );
                            }
                        }
                        RuntimeAction::Open { targets } => {
                            let urls = targets
                                .iter()
                                .map(|input| MediaUrl::from_input(input, &cwd))
                                .collect::<Result<Vec<_>, _>>();
                            match urls {
                                Ok(urls) => {
                                    match resolve_media_urls(urls) {
                                        Ok((resolved_urls, start_idx)) => {
                                            let mut new_animations = Vec::new();
                                            for url in resolved_urls {
                                                if let Ok(anim) = Animation::skim(url) {
                                                    new_animations.push(anim);
                                                }
                                            }
                                            
                                            animations = new_animations;
                                            if animations.is_empty() {
                                                warn!("No valid animations found, emitting empty status");
                                                let _ = app.emit("status", "No images provided.");
                                                let _ = app.emit("status-meta", StatusPayload {
                                                    current_index: 0,
                                                    total: 0,
                                                    display_text: "No images provided.".to_string(),
                                                    playback_state: "Paused".to_string(),
                                                });
                                                if let Some(window) = app.get_webview_window("main") {
                                                    let _ = window.set_title("viia - No images provided.");
                                                }
                                                // Reset everything so it renders empty state
                                                last_rendered_frame = usize::MAX;
                                            } else {
                                                info!("Opened {} new animations", animations.len());
                                                current_idx = start_idx.min(animations.len().saturating_sub(1));
                                                manager = SlideshowManager::new(default_cmd.clone());
                                                update_prefetch(&mut animations, current_idx, prefetch);
                                                animations[current_idx].ensure_parsed();

                                                if let Err(e) = manager.load_animation(&animations[current_idx]) {
                                                    animations[current_idx].state = viia_core::AnimationState::Error(e);
                                                }
                                                last_rendered_frame = usize::MAX;
                                            }
                                        }
                                        Err(e) => error!("Failed to resolve URLs: {}", e),
                                    }
                                }
                                Err(e) => error!("Failed to parse URLs: {}", e),
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

        if !animations.is_empty() {
            match manager.tick(dt, &animations[current_idx]) {
                Ok(should_advance) => {
                    if should_advance {
                        current_idx = (current_idx + 1) % animations.len();
                        info!(
                            "Automatically advancing to next animation: index {} ({})",
                            current_idx,
                            animations[current_idx].source.as_str()
                        );
                        update_prefetch(&mut animations, current_idx, prefetch);
                        if let Err(e) = manager.load_animation(&animations[current_idx]) {
                            animations[current_idx].state = viia_core::AnimationState::Error(e);
                        }
                        last_rendered_frame = usize::MAX;
                    }
                }
                Err(e) => {
                    animations[current_idx].state = viia_core::AnimationState::Error(e);
                }
            }
        }

        let frame_idx = manager.current_frame_index();
        let has_frame = manager.current_frame().is_some();
        let needs_render = current_idx != last_rendered_idx
            || frame_idx != last_rendered_frame
            || (!last_rendered_had_frame && has_frame);

        if needs_render {
            debug!(
                "Needs render triggered: current_idx={}, last_idx={}, frame_idx={}, last_frame={}",
                current_idx, last_rendered_idx, frame_idx, last_rendered_frame
            );

            if animations.is_empty() {
                last_rendered_idx = current_idx;
                last_rendered_frame = frame_idx;
                last_rendered_had_frame = has_frame;
            } else {
                let zoom_str = match &zoom_mode {
                viia_core::ZoomMode::Fit => "fit".to_string(),
                viia_core::ZoomMode::Shrink => "shrink".to_string(),
                viia_core::ZoomMode::Fixed(s) => s.to_string(),
            };

            if let Some(frame) = manager.current_frame() {
                if animations[current_idx].is_single_frame() {
                    let source = &animations[current_idx].source;
                    let image_path = source.as_str().to_string();
                    
                    let mut path = None;
                    
                    if source.scheme() == "file" {
                        if let Some(local_path) = source.to_file_path() {
                            path = Some(local_path.to_string_lossy().to_string());
                        }
                    }
                    
                    if let Some(valid_path) = path {
                        debug!(
                            "Emitting static frame for animation {} ({})",
                            current_idx, image_path
                        );
                        if let Err(e) = app.emit(
                            "frame",
                            FramePayload {
                                data_uri: None,
                                path: Some(valid_path),
                                blob_url: None,
                                zoom_mode: Some(zoom_str),
                                image_path: Some(image_path),
                                width: None,
                                height: None,
                            },
                        ) {
                            error!("Failed to emit frame event: {}", e);
                        }
                    } else {
                        // Serve raw RGBA bytes for static sftp or malformed files
                        let width = frame.data.width();
                        let height = frame.data.height();
                        let buffer = frame.data.as_raw().clone();

                        *frame_state.buffer.lock().unwrap() = buffer;
                        *frame_state.content_type.lock().unwrap() =
                            "application/octet-stream".to_string();

                        // Encode parameters for caching
                        let path_b64 = general_purpose::URL_SAFE_NO_PAD.encode(image_path.as_bytes());

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
                                image_path: Some(image_path),
                                width: Some(width),
                                height: Some(height),
                            },
                        ) {
                            error!("Failed to emit frame event: {}", e);
                        }
                    }
                } else {
                    // Serve raw RGBA bytes for animations
                    let width = frame.data.width();
                    let height = frame.data.height();
                    let buffer = frame.data.as_raw().clone();

                    *frame_state.buffer.lock().unwrap() = buffer;
                    *frame_state.content_type.lock().unwrap() =
                        "application/octet-stream".to_string();

                    // Encode parameters for caching
                    let path_str = animations[current_idx].source.as_str().to_string();
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
                            width: Some(width),
                            height: Some(height),
                        },
                    ) {
                        error!("Failed to emit frame event: {}", e);
                    }
                }
            } else if let viia_core::AnimationState::Error(err) = &animations[current_idx].state {
                error!("Failed to render image: {}", err);

                let path_str = animations[current_idx].source.as_str().to_string();
                
                // Use a 1x1 transparent raw RGBA buffer instead of base64
                let buffer = vec![0u8, 0, 0, 0];
                *frame_state.buffer.lock().unwrap() = buffer;
                *frame_state.content_type.lock().unwrap() = "application/octet-stream".to_string();
                
                let path_b64 = general_purpose::URL_SAFE_NO_PAD.encode(path_str.as_bytes());
                
                #[cfg(any(windows, target_os = "android"))]
                let protocol_prefix = "http://viia.localhost";
                #[cfg(not(any(windows, target_os = "android")))]
                let protocol_prefix = "viia://localhost";

                let blob_url = format!("{}/frame?path={}&frame=error", protocol_prefix, path_b64);

                if let Err(e) = app.emit(
                    "frame",
                    FramePayload {
                        data_uri: None,
                        path: None,
                        blob_url: Some(blob_url),
                        zoom_mode: Some(zoom_str),
                        image_path: Some(path_str),
                        width: Some(1),
                        height: Some(1),
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
                .source
                .file_name()
                .unwrap_or_else(|| "unknown".to_string());

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

            let status_payload = StatusPayload {
                current_index: zero_based_to_shell_index(current_idx),
                total: animations.len(),
                display_text: if let viia_core::AnimationState::Error(err) =
                    &animations[current_idx].state
                {
                    format!("{} (Error: {})", file_name, err)
                } else {
                    file_name.clone()
                },
                playback_state: if manager.state() == PlaybackState::Playing {
                    "Playing".to_string()
                } else {
                    "Paused".to_string()
                },
            };

            debug!("Emitting status update: {}", status_msg);
            if let Err(e) = app.emit("status", status_msg) {
                error!("Failed to emit status event: {}", e);
            }
            if let Err(e) = app.emit("status-meta", status_payload) {
                error!("Failed to emit status-meta event: {}", e);
            }

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("viia - {}", file_name));
            }

                last_rendered_idx = current_idx;
                last_rendered_frame = frame_idx;
                last_rendered_had_frame = has_frame;
            }
        }

        let sleep_dur = if animations.is_empty() {
            Duration::from_millis(100)
        } else {
            manager.time_until_next_frame(&animations[current_idx])
        };
        let poll_dur = sleep_dur.min(Duration::from_millis(16)); // ~60fps poll
        std::thread::sleep(poll_dur);
    }
}

pub fn run_gui(
    inputs: Vec<String>,
    dimension: Option<String>,
    prefetch: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(
        "Initializing Tauri GUI application with {} input paths",
        inputs.len()
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
        .plugin(tauri_plugin_dialog::init())
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
            log_to_terminal,
            open_file_dialog
        ])
        .setup(move |app| {
            info!("Tauri setup hook called, spawning engine loop thread");
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                engine_loop(handle, inputs, rx, frame_state, prefetch);
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
                let _ = tx_clone.send("g".to_string()).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    fn skim_animation(path: &std::path::Path) -> Animation {
        let source = MediaUrl::from_abs_path(path).unwrap();
        Animation::skim(source).unwrap()
    }

    #[test]
    fn test_match_file_names_matches_file_names_only() {
        let dir = tempdir().unwrap();
        let cat = dir.path().join("cat-01.png");
        let dog = dir.path().join("dog-01.png");
        let cat_jpg = dir.path().join("cat-02.jpg");
        File::create(&cat).unwrap();
        File::create(&dog).unwrap();
        File::create(&cat_jpg).unwrap();

        let animations = vec![
            skim_animation(&cat),
            skim_animation(&dog),
            skim_animation(&cat_jpg),
        ];

        let names = match_file_names(&animations, r"^cat.*\.(png|jpg)$").unwrap();
        assert_eq!(
            names,
            vec![(1, "cat-01.png".to_string()), (3, "cat-02.jpg".to_string())]
        );
    }

    #[test]
    fn test_match_file_names_rejects_invalid_regex() {
        let dir = tempdir().unwrap();
        let image = dir.path().join("image.png");
        File::create(&image).unwrap();

        let animations = vec![skim_animation(&image)];

        let err = match_file_names(&animations, "(").unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
