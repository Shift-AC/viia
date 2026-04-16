use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph},
};
use ratatui_image::{Resize, picker::Picker};
use regex::Regex;
use std::io::{self, Stdout};
use std::time::{Duration, Instant};
use tracing::{debug, error, warn};
use tui_logger::{TuiLoggerWidget, TuiWidgetState};
use viia_core::{
    Animation, FrameCache, MediaUrl, SlideshowManager, TimingCommand, resolve_media_urls,
    shell_index_to_zero_based, update_prefetch, zero_based_to_shell_index,
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

/// Main entrypoint for the TUI mode
pub fn run_tui(inputs: Vec<String>, prefetch: usize) -> Result<(), Box<dyn std::error::Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Initialize image picker (this probes the terminal for Sixel/Kitty/iTerm2 support)
    let mut picker = Picker::from_query_stdio()?;

    // Attempt to get the terminal background color
    let bg_color = terminal_light::background_color().unwrap_or_else(|_| {
        // Fallback to white if we can't query the background
        warn!("Failed to query terminal background color, using white as fallback");
        terminal_light::Color::Rgb(terminal_light::Rgb::new(255, 255, 255))
    });
    debug!("Terminal background color: {:?}", bg_color);

    let rgb = bg_color.rgb();
    let bg_rgba = [rgb.r, rgb.g, rgb.b, 255];

    // Set background color to terminal background to overwrite previous images when padding Sixel graphics
    picker.set_background_color(bg_rgba);
    let cache = FrameCache::default();

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

    // Main loop
    let res = if animations.is_empty() {
        // Just show empty state if no images
        run_empty_loop(&mut terminal)
    } else {
        run_loop(
            &mut terminal,
            picker,
            animations,
            cache,
            prefetch,
            start_idx,
        )
    };

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        error!("TUI error: {:?}", err);
    }

    Ok(())
}

fn run_empty_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            terminal.draw(|f| {
                let block = Block::default().title(" viia ").borders(Borders::ALL);
                let text = Paragraph::new("No images provided. Press 'q' to quit.")
                    .block(block)
                    .alignment(ratatui::layout::Alignment::Center);
                f.render_widget(text, f.area());
            })?;
            needs_redraw = false;
        }

        if event::poll(Duration::from_millis(100))? {
            let ev = event::read()?;
            needs_redraw = true;
            if let Event::Key(key) = ev
                && key.kind == KeyEventKind::Press
                && key.code == KeyCode::Char('q')
            {
                break;
            }
        }
    }
    Ok(())
}

#[derive(PartialEq)]
enum InputMode {
    Normal,
    Command,
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    picker: Picker,
    mut animations: Vec<Animation>,
    cache: FrameCache,
    prefetch: usize,
    start_idx: usize,
) -> io::Result<()> {
    let mut current_idx = start_idx.min(animations.len().saturating_sub(1));

    update_prefetch(&mut animations, current_idx, prefetch);
    animations[current_idx].ensure_parsed();
    // Default to loop forever
    let default_cmd = vec![TimingCommand {
        loops: None,
        time_secs: None,
        infinite: true,
    }];
    let mut manager = SlideshowManager::new(default_cmd.clone());
    if let Err(e) = manager.load_animation(&animations[current_idx]) {
        animations[current_idx].state = viia_core::AnimationState::Error(e);
    }

    terminal.clear()?;

    let mut last_tick = Instant::now();
    let mut image_state: Option<ratatui_image::protocol::StatefulProtocol> = None;
    let mut last_rendered_frame = usize::MAX;
    let mut last_rendered_idx = usize::MAX;
    let mut last_rendered_had_frame = false;

    let mut log_height: u16 = 10;
    let mut is_dragging = false;
    let tui_logger_state = TuiWidgetState::new();

    let mut input_mode = InputMode::Normal;
    let mut command_input = String::new();
    let mut needs_redraw = true;
    let mut force_clear = false;
    let mut zoom_mode = viia_core::ZoomMode::Fit;

    loop {
        let now = Instant::now();
        let dt = now.duration_since(last_tick);
        last_tick = now;

        update_prefetch(&mut animations, current_idx, prefetch);

        match manager.tick(dt, &animations[current_idx]) {
            Ok(should_advance) => {
                if should_advance {
                    current_idx = (current_idx + 1) % animations.len();
                    tracing::info!(
                        "Auto-advancing to next image: {}",
                        animations[current_idx].source.as_str()
                    );
                    update_prefetch(&mut animations, current_idx, prefetch);
                    if let Err(e) = manager.load_animation(&animations[current_idx]) {
                        animations[current_idx].state = viia_core::AnimationState::Error(e);
                    }
                    last_rendered_frame = usize::MAX; // Force re-render
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
            if current_idx != last_rendered_idx {
                force_clear = true;
            }
            needs_redraw = true;
            if let Some(frame) = manager.current_frame() {
                let dyn_img = match zoom_mode {
                    viia_core::ZoomMode::Fixed(scale) => {
                        let new_width = (frame.data.width() as f32 * scale / 100.0).max(1.0) as u32;
                        let new_height =
                            (frame.data.height() as f32 * scale / 100.0).max(1.0) as u32;

                        let key = viia_core::CacheKey {
                            source: animations[current_idx].source.clone(),
                            frame_index: frame_idx,
                            target_width: new_width,
                            target_height: new_height,
                        };

                        if let Some(resized) = cache.get_or_resize(key, &frame.data) {
                            image::DynamicImage::ImageRgba8((*resized).clone())
                        } else {
                            image::DynamicImage::ImageRgba8(frame.data.clone())
                        }
                    }
                    _ => image::DynamicImage::ImageRgba8(frame.data.clone()),
                };

                let protocol = picker.new_resize_protocol(dyn_img);
                image_state = Some(protocol);
            } else if let viia_core::AnimationState::Error(err) = &animations[current_idx].state {
                tracing::error!("Failed to render image: {}", err);
                image_state = None;
            } else {
                image_state = None;
            }
        }

        if needs_redraw {
            if force_clear {
                terminal.clear()?;
                force_clear = false;
            }
            terminal.draw(|f| {
                let term_height = f.area().height;
                // Ensure log_height is valid (at least 2 for border + text, and leave at least 3 for image and status)
                log_height = log_height.clamp(2, term_height.saturating_sub(3).max(2));

                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(0)
                    .constraints([
                        Constraint::Min(0), // Image area
                        Constraint::Length(log_height), // Log area
                        Constraint::Length(1), // Command/Status area
                    ])
                    .split(f.area());

                let img_area = chunks[0];
                f.render_widget(Clear, img_area);

                if let Some(state) = &mut image_state {
                    let resize_mode = match zoom_mode {
                        viia_core::ZoomMode::Fit => Resize::Scale(None),
                        viia_core::ZoomMode::Shrink => Resize::Fit(None),
                        viia_core::ZoomMode::Fixed(_) => Resize::Crop(None),
                    };
                    let image_widget = ratatui_image::StatefulImage::new().resize(resize_mode);
                    f.render_stateful_widget(image_widget, img_area, state);
                }

                let log_widget = TuiLoggerWidget::default()
                    .block(Block::default().borders(Borders::TOP).title(" Log "))
                    .style_error(Style::default().fg(Color::Red))
                    .style_warn(Style::default().fg(Color::Yellow))
                    .style_debug(Style::default().fg(Color::Blue))
                    .style_trace(Style::default().fg(Color::Magenta))
                    .state(&tui_logger_state);
                f.render_widget(log_widget, chunks[1]);

                let status_text = match input_mode {
                    InputMode::Normal => {
                        let error_msg = if let viia_core::AnimationState::Error(err) = &animations[current_idx].state {
                            format!(" (Error: {})", err)
                        } else {
                            String::new()
                        };
                        Paragraph::new(format!(
                            " [{}/{}] {}{} | Press 'q' to quit, Space to pause, Right next, Left prev, ':' command ",
                            current_idx + 1,
                            animations.len(),
                            animations[current_idx]
                                .source
                                .file_name()
                                .unwrap_or_else(|| "unknown".to_string()),
                            error_msg
                        ))
                        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
                    },
                    InputMode::Command => Paragraph::new(format!(":{}", command_input))
                    .style(Style::default().bg(Color::Blue).fg(Color::White)),
                };

                f.render_widget(status_text, chunks[2]);
            })?;
            needs_redraw = false;
        }

        if needs_render {
            last_rendered_idx = current_idx;
            last_rendered_frame = frame_idx;
            last_rendered_had_frame = has_frame;
        }

        let sleep_dur = manager.time_until_next_frame(&animations[current_idx]);
        // Cap sleep at 50ms so we remain responsive to keystrokes
        let poll_dur = sleep_dur.min(Duration::from_millis(50));

        if event::poll(poll_dur)? {
            let ev = event::read()?;
            needs_redraw = true; // Any event might require redrawing
            match ev {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        match input_mode {
                            InputMode::Normal => match key.code {
                                KeyCode::Char('q') => break,
                                KeyCode::Char(' ') => manager.toggle_pause(),
                                KeyCode::Right => {
                                    current_idx = (current_idx + 1) % animations.len();
                                    tracing::info!(
                                        "Manually navigated to next image: {}",
                                        animations[current_idx].source.as_str()
                                    );
                                    update_prefetch(&mut animations, current_idx, prefetch);
                                    if let Err(e) = manager.load_animation(&animations[current_idx])
                                    {
                                        animations[current_idx].state =
                                            viia_core::AnimationState::Error(e);
                                    }
                                    last_rendered_frame = usize::MAX;
                                }
                                KeyCode::Left => {
                                    if current_idx == 0 {
                                        current_idx = animations.len() - 1;
                                    } else {
                                        current_idx -= 1;
                                    }
                                    tracing::info!(
                                        "Manually navigated to previous image: {}",
                                        animations[current_idx].source.as_str()
                                    );
                                    update_prefetch(&mut animations, current_idx, prefetch);
                                    if let Err(e) = manager.load_animation(&animations[current_idx])
                                    {
                                        animations[current_idx].state =
                                            viia_core::AnimationState::Error(e);
                                    }
                                    last_rendered_frame = usize::MAX;
                                }
                                KeyCode::Char(':') => {
                                    tracing::info!("Entering command mode");
                                    input_mode = InputMode::Command;
                                    command_input.clear();
                                }
                                _ => {}
                            },
                            InputMode::Command => {
                                match key.code {
                                    KeyCode::Enter => {
                                        let line = command_input.trim();
                                        if !line.is_empty() {
                                            match viia_core::InternalCommand::parse_line(line) {
                                                Ok(cmd) => match cmd.action {
                                                    viia_core::RuntimeAction::Quit => {
                                                        break;
                                                    }
                                                    viia_core::RuntimeAction::TogglePause => {
                                                        manager.toggle_pause();
                                                    }
                                                    viia_core::RuntimeAction::ShowNext => {
                                                        current_idx =
                                                            (current_idx + 1) % animations.len();
                                                        tracing::info!(
                                                            "Navigated to next image: {}",
                                                            animations[current_idx].source.as_str()
                                                        );
                                                        update_prefetch(
                                                            &mut animations,
                                                            current_idx,
                                                            prefetch,
                                                        );
                                                        if let Err(e) = manager.load_animation(
                                                            &animations[current_idx],
                                                        ) {
                                                            animations[current_idx].state =
                                                                viia_core::AnimationState::Error(e);
                                                        }
                                                        last_rendered_frame = usize::MAX;
                                                    }
                                                    viia_core::RuntimeAction::ShowPrevious => {
                                                        if current_idx == 0 {
                                                            current_idx = animations.len() - 1;
                                                        } else {
                                                            current_idx -= 1;
                                                        }
                                                        tracing::info!(
                                                            "Navigated to previous image: {}",
                                                            animations[current_idx].source.as_str()
                                                        );
                                                        update_prefetch(
                                                            &mut animations,
                                                            current_idx,
                                                            prefetch,
                                                        );
                                                        if let Err(e) = manager.load_animation(
                                                            &animations[current_idx],
                                                        ) {
                                                            animations[current_idx].state =
                                                                viia_core::AnimationState::Error(e);
                                                        }
                                                        last_rendered_frame = usize::MAX;
                                                    }
                                                    viia_core::RuntimeAction::Goto { index } => {
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
                                                            update_prefetch(
                                                                &mut animations,
                                                                current_idx,
                                                                prefetch,
                                                            );
                                                            if let Err(e) = manager.load_animation(
                                                                &animations[current_idx],
                                                            ) {
                                                                animations[current_idx].state =
                                                                    viia_core::AnimationState::Error(e);
                                                            }
                                                            last_rendered_frame = usize::MAX;
                                                        }
                                                        tracing::info!(
                                                            "Current image [{}]: {}",
                                                            zero_based_to_shell_index(current_idx),
                                                            animations[current_idx].source.as_str()
                                                        );
                                                    }
                                                    viia_core::RuntimeAction::Match { pattern } => {
                                                        match match_file_names(
                                                            &animations,
                                                            &pattern,
                                                        ) {
                                                            Ok(names) => {
                                                                tracing::info!(
                                                                    "Matching current file list with regex: {}",
                                                                    pattern
                                                                );
                                                                if names.is_empty() {
                                                                    tracing::info!(
                                                                        "No files matched regex: {}",
                                                                        pattern
                                                                    );
                                                                } else {
                                                                    for (idx, name) in &names {
                                                                        tracing::info!(
                                                                            "{} {}", idx, name
                                                                        );
                                                                    }
                                                                    tracing::info!(
                                                                        "Matched {} file(s) for regex: {}",
                                                                        names.len(),
                                                                        pattern
                                                                    );
                                                                }
                                                            }
                                                            Err(err) => {
                                                                error!(
                                                                    "Invalid regex '{}': {}",
                                                                    pattern, err
                                                                );
                                                            }
                                                        }
                                                    }
                                                    viia_core::RuntimeAction::Dimension { dim } => {
                                                        tracing::info!(
                                                            "Dimension command received: {:?}",
                                                            dim
                                                        );
                                                    }
                                                    viia_core::RuntimeAction::Zoom { mode } => {
                                                        tracing::info!(
                                                            "Zoom command received: {:?}",
                                                            mode
                                                        );
                                                        zoom_mode = mode;
                                                        force_clear = true;
                                                        last_rendered_frame = usize::MAX; // force redraw
                                                    }
                                                    viia_core::RuntimeAction::StartSlideshow {
                                                        cmd,
                                                    } => {
                                                        let spec = cmd.join(" ");
                                                        let parent_dir_path = animations
                                                            [current_idx]
                                                            .source
                                                            .to_file_path()
                                                            .and_then(|p| {
                                                                p.parent().map(|x| x.to_path_buf())
                                                            });
                                                        let parent_dir = parent_dir_path
                                                            .as_deref()
                                                            .unwrap_or(std::path::Path::new(""));
                                                        if let Ok(cmds) =
                                                            viia_core::parse_slideshow_spec(
                                                                &spec, parent_dir,
                                                            )
                                                        {
                                                            if !cmds.is_empty() {
                                                                if let Err(e) = manager
                                                                    .set_commands(
                                                                        cmds,
                                                                        &animations[current_idx],
                                                                    )
                                                                {
                                                                    animations[current_idx].state = viia_core::AnimationState::Error(e);
                                                                }
                                                                last_rendered_frame = usize::MAX;
                                                            }
                                                        } else {
                                                            error!(
                                                                "Failed to parse slideshow spec: {}",
                                                                spec
                                                            );
                                                        }
                                                    }
                                                    viia_core::RuntimeAction::Open { targets } => {
                                                        let cwd = std::env::current_dir()
                                                            .unwrap_or_else(|_| std::path::PathBuf::from("."));
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
                                                                        if new_animations.is_empty() {
                                                                            error!("No images found from provided targets");
                                                                        } else {
                                                                            animations = new_animations;
                                                                            current_idx = start_idx.min(animations.len().saturating_sub(1));
                                                                            // FrameCache isn't easily cleared here without re-creating it, but we can just let old items expire.
                                                                            manager = SlideshowManager::new(default_cmd.clone());
                                                                            if let Err(e) = manager.load_animation(&animations[current_idx]) {
                                                                                animations[current_idx].state = viia_core::AnimationState::Error(e);
                                                                            }
                                                                            force_clear = true;
                                                                            last_rendered_frame = usize::MAX;
                                                                            tracing::info!("Opened {} new images", animations.len());
                                                                        }
                                                                    }
                                                                    Err(e) => error!("Failed to resolve URLs: {}", e),
                                                                }
                                                            }
                                                            Err(e) => error!("Failed to parse URLs: {}", e),
                                                        }
                                                    }
                                                    viia_core::RuntimeAction::Help => unreachable!(
                                                        "Help is handled internally by parse_line"
                                                    ),
                                                },
                                                Err(e) => {
                                                    if e.kind()
                                                        == clap::error::ErrorKind::DisplayHelp
                                                    {
                                                        tracing::info!("{}", e);
                                                    } else {
                                                        error!("Invalid command: {}", e);
                                                    }
                                                }
                                            }
                                        }
                                        input_mode = InputMode::Normal;
                                    }
                                    KeyCode::Esc => {
                                        input_mode = InputMode::Normal;
                                    }
                                    KeyCode::Char(c) => {
                                        command_input.push(c);
                                    }
                                    KeyCode::Backspace => {
                                        command_input.pop();
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if let Ok(size) = terminal.size() {
                                // The border is at y = size.height - log_height - 1 (status bar)
                                let border_y =
                                    size.height.saturating_sub(log_height).saturating_sub(1);
                                if mouse.row == border_y {
                                    is_dragging = true;
                                }
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if is_dragging && let Ok(size) = terminal.size() {
                                // New log height is from mouse.row down to the status bar
                                // So size.height - 1 (status bar) - mouse.row
                                let new_height =
                                    size.height.saturating_sub(mouse.row).saturating_sub(1);
                                log_height =
                                    new_height.clamp(2, size.height.saturating_sub(3).max(2));
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            is_dragging = false;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
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
}
