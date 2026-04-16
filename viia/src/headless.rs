use std::io::{self, BufRead};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use tracing::{error, info};
use viia_core::{
    Animation, InternalCommand, MediaUrl, RuntimeAction, SlideshowManager, TimingCommand,
    parse_slideshow_spec, resolve_media_urls, shell_index_to_zero_based, update_prefetch,
    zero_based_to_shell_index,
};

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

fn goto_index(
    current_idx: usize,
    requested_idx: Option<usize>,
    len: usize,
) -> Result<usize, String> {
    match requested_idx {
        Some(idx) => {
            let zero_based_idx = shell_index_to_zero_based(idx)?;
            if zero_based_idx >= len {
                Err(format!(
                    "File index {} is out of range for file list of length {}",
                    idx, len
                ))
            } else {
                Ok(zero_based_idx)
            }
        }
        None => Ok(current_idx),
    }
}

pub fn run_headless(
    inputs: Vec<String>,
    prefetch: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let urls = inputs
        .iter()
        .map(|input| MediaUrl::from_input(input, &cwd))
        .collect::<Result<Vec<_>, _>>()?;
    let (resolved_urls, start_idx) = resolve_media_urls(urls)?;
    let mut animations = Vec::new();
    for url in resolved_urls {
        if let Ok(anim) = Animation::skim(url) {
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
                                if animations.is_empty() {
                                    continue;
                                }
                                current_idx = (current_idx + 1) % animations.len();
                                info!(
                                    "Navigated to next image: {}",
                                    animations[current_idx].source.as_str()
                                );
                                update_prefetch(&mut animations, current_idx, prefetch);
                                if let Err(e) = manager.load_animation(&animations[current_idx]) {
                                    animations[current_idx].state =
                                        viia_core::AnimationState::Error(e);
                                }
                                last_rendered_frame = usize::MAX;
                            }
                            RuntimeAction::ShowPrevious => {
                                if animations.is_empty() {
                                    continue;
                                }
                                if current_idx == 0 {
                                    current_idx = animations.len() - 1;
                                } else {
                                    current_idx -= 1;
                                }
                                info!(
                                    "Navigated to previous image: {}",
                                    animations[current_idx].source.as_str()
                                );
                                update_prefetch(&mut animations, current_idx, prefetch);
                                if let Err(e) = manager.load_animation(&animations[current_idx]) {
                                    animations[current_idx].state =
                                        viia_core::AnimationState::Error(e);
                                }
                                last_rendered_frame = usize::MAX;
                            }
                            RuntimeAction::Goto { index } => {
                                if animations.is_empty() {
                                    continue;
                                }
                                match goto_index(current_idx, index, animations.len()) {
                                    Ok(target_idx) => {
                                        current_idx = target_idx;
                                        info!(
                                            "Current image [{}]: {}",
                                            zero_based_to_shell_index(current_idx),
                                            animations[current_idx].source.as_str()
                                        );
                                        update_prefetch(&mut animations, current_idx, prefetch);
                                        if let Err(e) =
                                            manager.load_animation(&animations[current_idx])
                                        {
                                            animations[current_idx].state =
                                                viia_core::AnimationState::Error(e);
                                        }
                                        last_rendered_frame = usize::MAX;
                                    }
                                    Err(err) => error!("{err}"),
                                }
                            }
                            RuntimeAction::Match { pattern } => {
                                match match_file_names(&animations, &pattern) {
                                    Ok(names) => {
                                        info!("Matching current file list with regex: {}", pattern);
                                        if names.is_empty() {
                                            info!("No files matched regex: {}", pattern);
                                        } else {
                                            for (idx, name) in &names {
                                                println!("{idx} {name}");
                                            }
                                            info!(
                                                "Matched {} file(s) for regex: {}",
                                                names.len(),
                                                pattern
                                            );
                                        }
                                    }
                                    Err(err) => {
                                        error!("Invalid regex '{}': {}", pattern, err);
                                    }
                                }
                            }
                            RuntimeAction::Dimension { dim } => {
                                info!("Dimension command received: {:?}", dim);
                                // Headless mode doesn't actually resize a window
                            }
                            RuntimeAction::Zoom { mode } => {
                                info!("Zoom command received: {:?}", mode);
                                // Headless mode doesn't actually scale a window
                            }
                            viia_core::RuntimeAction::StartSlideshow { cmd } => {
                                if animations.is_empty() {
                                    continue;
                                }
                                let spec = cmd.join(" ");
                                let parent_dir_path = animations[current_idx]
                                    .source
                                    .to_file_path()
                                    .and_then(|p| p.parent().map(|x| x.to_path_buf()));
                                let parent_dir = parent_dir_path
                                    .as_deref()
                                    .unwrap_or(std::path::Path::new(""));
                                if let Ok(cmds) = parse_slideshow_spec(&spec, parent_dir) {
                                    if !cmds.is_empty() {
                                        info!("Starting slideshow with spec: {}", spec);
                                        if let Err(e) =
                                            manager.set_commands(cmds, &animations[current_idx])
                                        {
                                            animations[current_idx].state =
                                                viia_core::AnimationState::Error(e);
                                        }
                                        last_rendered_frame = usize::MAX;
                                    }
                                } else {
                                    error!("Failed to parse slideshow spec: {}", spec);
                                }
                            }
                            RuntimeAction::Open { targets } => {
                                info!("Open command received with targets: {:?}", targets);
                                let cwd = std::env::current_dir()
                                    .unwrap_or_else(|_| std::path::PathBuf::from("."));
                                let urls = targets
                                    .iter()
                                    .map(|input| MediaUrl::from_input(input, &cwd))
                                    .collect::<Result<Vec<_>, _>>();
                                match urls {
                                    Ok(urls) => match resolve_media_urls(urls) {
                                        Ok((resolved_urls, start_idx)) => {
                                            let mut new_animations = Vec::new();
                                            for url in resolved_urls {
                                                if let Ok(anim) = Animation::skim(url) {
                                                    new_animations.push(anim);
                                                }
                                            }

                                            animations = new_animations;
                                            if animations.is_empty() {
                                                error!("No images found from provided targets");
                                            } else {
                                                info!("Opened {} new images", animations.len());
                                                current_idx = start_idx
                                                    .min(animations.len().saturating_sub(1));
                                                manager =
                                                    SlideshowManager::new(default_cmd.clone());
                                                update_prefetch(
                                                    &mut animations,
                                                    current_idx,
                                                    prefetch,
                                                );
                                                animations[current_idx].ensure_parsed();

                                                if let Err(e) =
                                                    manager.load_animation(&animations[current_idx])
                                                {
                                                    animations[current_idx].state =
                                                        viia_core::AnimationState::Error(e);
                                                }
                                                last_rendered_frame = usize::MAX;
                                            }
                                        }
                                        Err(e) => error!("Failed to resolve URLs: {}", e),
                                    },
                                    Err(e) => error!("Failed to parse URLs: {}", e),
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

        if !animations.is_empty() {
            match manager.tick(dt, &animations[current_idx]) {
                Ok(should_advance) => {
                    if should_advance {
                        current_idx = (current_idx + 1) % animations.len();
                        info!(
                            "Auto-advancing to next image: {}",
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
            if animations.is_empty() {
                last_rendered_idx = current_idx;
                last_rendered_frame = frame_idx;
                last_rendered_had_frame = has_frame;
            } else {
                let total_frames = match &animations[current_idx].state {
                    viia_core::AnimationState::Static { .. } => "1".to_string(),
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
                        animations[current_idx].source.as_str()
                    );
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
        thread::sleep(poll_dur);
    }

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

    #[test]
    fn test_goto_index_uses_current_index_when_omitted() {
        assert_eq!(goto_index(2, None, 5).unwrap(), 2);
    }

    #[test]
    fn test_goto_index_uses_requested_index_when_in_range() {
        assert_eq!(goto_index(2, Some(5), 5).unwrap(), 4);
    }

    #[test]
    fn test_goto_index_rejects_zero_index() {
        let err = goto_index(2, Some(0), 5).unwrap_err();
        assert!(err.contains("at least 1"));
    }

    #[test]
    fn test_goto_index_rejects_out_of_range_index() {
        let err = goto_index(2, Some(6), 5).unwrap_err();
        assert!(err.contains("out of range"));
    }
}
