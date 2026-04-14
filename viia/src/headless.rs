use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tracing::{error, info};
use viia_core::{
    Animation, InternalCommand, RuntimeAction, SlideshowManager, TimingCommand,
    collect_image_paths, parse_slideshow_spec, update_prefetch,
};

pub fn run_headless(paths: Vec<PathBuf>, prefetch: usize) -> Result<(), Box<dyn std::error::Error>> {
    let (resolved_paths, start_idx) = collect_image_paths(paths);
    let mut animations = Vec::new();
    for path in resolved_paths {
        if let Ok(anim) = Animation::skim(path) {
            animations.push(anim);
        }
    }

    if animations.is_empty() {
        info!("No images provided. Exiting.");
        return Ok(());
    }

    let mut current_idx = start_idx.min(animations.len().saturating_sub(1));
    update_prefetch(&mut animations, current_idx, prefetch);
    animations[current_idx].ensure_parsed();

    let default_cmd = vec![TimingCommand {
        loops: None,
        time_secs: None,
        infinite: true,
    }];
    let mut manager = SlideshowManager::new(default_cmd.clone());
    if let Err(e) = manager.load_animation(&animations[current_idx]) {
        animations[current_idx].state = viia_core::AnimationState::Error(e);
    }

    ctrlc::set_handler(move || {
        info!("Received Ctrl+C, exiting headless mode.");
        std::process::exit(0);
    })
    .unwrap_or_else(|err| error!("Error setting Ctrl-C handler: {}", err));

    // Set up a channel to receive stdin lines without blocking the main loop
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        let mut line = String::new();
        while handle.read_line(&mut line).is_ok() {
            if line.is_empty() {
                break; // EOF
            }
            if tx.send(line.clone()).is_err() {
                break;
            }
            line.clear();
        }
    });

    info!("Headless mode started. Waiting for commands on stdin...");

    let mut last_tick = Instant::now();
    let mut last_rendered_frame = usize::MAX;
    let mut last_rendered_idx = usize::MAX;
    let mut last_rendered_had_frame = false;

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_tick);
        last_tick = now;

        update_prefetch(&mut animations, current_idx, prefetch);

        // Process stdin commands
        match rx.try_recv() {
            Ok(line) => {
                let line = line.trim();
                if !line.is_empty() {
                    match InternalCommand::parse_line(line) {
                        Ok(cmd) => match cmd.action {
                            RuntimeAction::Quit => {
                                info!("Quit command received. Exiting.");
                                break;
                            }
                            RuntimeAction::TogglePause => {
                                manager.toggle_pause();
                                info!("Playback state: {:?}", manager.state());
                            }
                            RuntimeAction::ShowNext => {
                                current_idx = (current_idx + 1) % animations.len();
                                info!(
                                    "Navigated to next image: {}",
                                    animations[current_idx].source_path.display()
                                );
                                update_prefetch(&mut animations, current_idx, prefetch);
                                if let Err(e) = manager.load_animation(&animations[current_idx]) {
                                    animations[current_idx].state = viia_core::AnimationState::Error(e);
                                }
                                last_rendered_frame = usize::MAX;
                            }
                            RuntimeAction::ShowPrevious => {
                                if current_idx == 0 {
                                    current_idx = animations.len() - 1;
                                } else {
                                    current_idx -= 1;
                                }
                                info!(
                                    "Navigated to previous image: {}",
                                    animations[current_idx].source_path.display()
                                );
                                update_prefetch(&mut animations, current_idx, prefetch);
                                if let Err(e) = manager.load_animation(&animations[current_idx]) {
                                    animations[current_idx].state = viia_core::AnimationState::Error(e);
                                }
                                last_rendered_frame = usize::MAX;
                            }
                            RuntimeAction::ShowCurrent => {
                                info!(
                                    "Current image: {}",
                                    animations[current_idx].source_path.display()
                                );
                            }
                            RuntimeAction::Dimension { dim } => {
                                info!("Dimension command received: {:?}", dim);
                                // Headless mode doesn't actually resize a window
                            }
                            RuntimeAction::Zoom { mode } => {
                                info!("Zoom command received: {:?}", mode);
                                // Headless mode doesn't actually scale a window
                            }
                            RuntimeAction::StartSlideshow { cmd } => {
                                let spec = cmd.join(" ");
                                let parent_dir = animations[current_idx]
                                    .source_path
                                    .parent()
                                    .unwrap_or(std::path::Path::new(""));
                                if let Ok(cmds) = parse_slideshow_spec(&spec, parent_dir) {
                                    if !cmds.is_empty() {
                                        info!("Starting slideshow with spec: {}", spec);
                                        if let Err(e) = manager.set_commands(cmds, &animations[current_idx]) {
                                            animations[current_idx].state = viia_core::AnimationState::Error(e);
                                        }
                                        last_rendered_frame = usize::MAX;
                                    }
                                } else {
                                    error!("Failed to parse slideshow spec: {}", spec);
                                }
                            }
                            RuntimeAction::Help => {
                                unreachable!("Help is handled internally by parse_line")
                            }
                        },
                        Err(e) => {
                            if e.kind() == clap::error::ErrorKind::DisplayHelp {
                                info!("{}", e);
                            } else {
                                error!("Invalid command: {}", e);
                            }
                        }
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                // No command, just continue
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                info!("Stdin closed or disconnected. Exiting headless mode.");
                break;
            }
        }

        match manager.tick(dt, &animations[current_idx]) {
            Ok(should_advance) => {
                if should_advance {
                    current_idx = (current_idx + 1) % animations.len();
                    info!(
                        "Auto-advancing to next image: {}",
                        animations[current_idx].source_path.display()
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

        let frame_idx = manager.current_frame_index();
        let has_frame = manager.current_frame().is_some();
        let needs_render = current_idx != last_rendered_idx 
            || frame_idx != last_rendered_frame 
            || (!last_rendered_had_frame && has_frame);

        if needs_render {
            let total_frames = match &animations[current_idx].state {
                viia_core::AnimationState::Static(_) => "1".to_string(),
                viia_core::AnimationState::Animated { .. } => "?".to_string(),
                _ => "0".to_string(),
            };
            
            if let viia_core::AnimationState::Error(err) = &animations[current_idx].state {
                error!("Failed to render image: {}", err);
            } else if manager.current_frame().is_some() {
                // In headless mode, "rendering" is just logging the frame change
                info!(
                    "Rendering frame {}/{} of {}",
                    frame_idx + 1,
                    total_frames,
                    animations[current_idx].source_path.display()
                );
            }
            last_rendered_idx = current_idx;
            last_rendered_frame = frame_idx;
            last_rendered_had_frame = has_frame;
        }

        let sleep_dur = manager.time_until_next_frame(&animations[current_idx]);
        let poll_dur = sleep_dur.min(Duration::from_millis(50));
        thread::sleep(poll_dur);
    }

    Ok(())
}
