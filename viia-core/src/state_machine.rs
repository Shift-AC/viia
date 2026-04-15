use crate::lazy_decoder::LazyDecoder;
use crate::slideshow_parser::TimingCommand;
use crate::{Animation, AnimationState, Frame};
use std::time::{Duration, Instant};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
}

pub struct SlideshowManager {
    /// The parsed timing commands for the slideshow. Used cyclically.
    pub commands: Vec<TimingCommand>,
    /// Index of the current command
    current_command_idx: usize,

    /// State tracking
    state: PlaybackState,
    stored_state: Option<PlaybackState>,
    start_time: Option<Instant>,
    elapsed_time: Duration,

    /// Frame and Decoder state
    current_frame: Option<Frame>,
    current_frame_idx: usize,
    frame_elapsed: Duration,

    active_decoder: Option<LazyDecoder>,
    loops_completed: u32,
}

impl SlideshowManager {
    pub fn new(commands: Vec<TimingCommand>) -> Self {
        Self {
            commands,
            current_command_idx: 0,
            state: PlaybackState::Playing,
            stored_state: None,
            start_time: None,
            elapsed_time: Duration::ZERO,
            current_frame: None,
            current_frame_idx: 0,
            frame_elapsed: Duration::ZERO,
            active_decoder: None,
            loops_completed: 0,
        }
    }

    /// Sets a new set of timing commands and resets the current state
    pub fn set_commands(
        &mut self,
        commands: Vec<TimingCommand>,
        current_animation: &Animation,
    ) -> Result<(), String> {
        info!("Setting new commands: {:?}", commands);
        self.commands = commands;
        self.current_command_idx = 0;
        self.state = PlaybackState::Playing;
        self.load_animation(current_animation)
    }

    /// Initializes the manager for a new animation
    pub fn load_animation(&mut self, animation: &Animation) -> Result<(), String> {
        self.active_decoder = None;
        self.current_frame = None;
        self.current_frame_idx = 0;
        self.loops_completed = 0;
        self.frame_elapsed = Duration::ZERO;
        self.elapsed_time = Duration::ZERO;

        let mut load_err = None;

        match &animation.state {
            AnimationState::Static(frame) => {
                self.current_frame = Some(frame.clone());
            }
            AnimationState::Animated {
                bytes,
                format,
                first_frame,
            } => {
                // Use the pre-decoded first frame immediately
                self.current_frame = Some(first_frame.clone());

                // Initialize the decoder but do NOT call next() yet.
                // tick() will handle advancing when the first frame's duration expires.
                match LazyDecoder::new(bytes.clone(), *format) {
                    Ok(mut decoder) => {
                        // Skip the first frame in the decoder since we already have it
                        let _ = decoder.next();
                        self.active_decoder = Some(decoder);
                    }
                    Err(e) => {
                        warn!("Failed to initialize lazy decoder: {}", e);
                        load_err = Some(e.to_string());
                    }
                }
            }
            AnimationState::Parsing(_, _) | AnimationState::Skimmed => {}
            AnimationState::Error(e) => {
                load_err = Some(e.clone());
            }
        }

        if let Some(err) = load_err {
            if self.stored_state.is_none() {
                self.stored_state = Some(self.state);
            }
            self.state = PlaybackState::Paused; // Stop playing if it's broken
            return Err(err);
        }

        if let Some(state) = self.stored_state.take() {
            self.state = state;
        }

        let cmd = self.get_current_command().clone();
        info!("Loaded animation: {:?}", animation.source_path);
        info!("Current command: {:?}", cmd);

        if self.state == PlaybackState::Playing {
            self.start_time = Some(Instant::now());
        } else {
            self.start_time = None;
        }

        Ok(())
    }

    /// Gets the current command, cycling through the list if necessary
    fn get_current_command(&self) -> &TimingCommand {
        if self.commands.is_empty() {
            // Fallback default: Loop 1
            static DEFAULT_CMD: TimingCommand = TimingCommand {
                loops: Some(1),
                time_secs: None,
                infinite: false,
            };
            return &DEFAULT_CMD;
        }
        &self.commands[self.current_command_idx % self.commands.len()]
    }

    pub fn toggle_pause(&mut self) {
        if let Some(stored) = self.stored_state.as_mut() {
            *stored = match *stored {
                PlaybackState::Playing => PlaybackState::Paused,
                PlaybackState::Paused => PlaybackState::Playing,
            };
            info!(
                "Playback state toggled while in error state to {:?}",
                *stored
            );
            return;
        }

        match self.state {
            PlaybackState::Playing => {
                self.update_elapsed();
                self.state = PlaybackState::Paused;
                self.start_time = None;
                info!("Playback paused");
            }
            PlaybackState::Paused => {
                self.state = PlaybackState::Playing;
                self.start_time = Some(Instant::now());
                info!("Playback resumed");
            }
        }
    }

    fn update_elapsed(&mut self) {
        if let Some(start) = self.start_time {
            let now = Instant::now();
            let diff = now.duration_since(start);
            self.elapsed_time += diff;
            self.frame_elapsed += diff;
            self.start_time = Some(now);
        }
    }

    fn should_advance_animation(&self) -> bool {
        let cmd = self.get_current_command();
        if cmd.infinite {
            return false;
        }

        let mut ready_loops = false;
        let mut ready_time = false;

        if let Some(l) = cmd.loops {
            if self.loops_completed >= l {
                ready_loops = true;
            }
        } else {
            ready_loops = true; // No loop requirement
        }

        if let Some(t) = cmd.time_secs {
            if self.elapsed_time >= Duration::from_secs(t as u64) {
                ready_time = true;
            }
        } else {
            ready_time = true; // No time requirement
        }

        // If neither is specified, default is 1 loop
        if cmd.loops.is_none() && cmd.time_secs.is_none() && self.loops_completed >= 1 {
            return true;
        }

        ready_loops && ready_time
    }

    /// Ticks the state machine by `dt`.
    /// Returns `true` if the manager should advance to the next image in the list.
    pub fn tick(&mut self, dt: Duration, animation: &Animation) -> Result<bool, String> {
        if self.state == PlaybackState::Paused {
            return Ok(false);
        }

        // If the animation just finished parsing in the background, load it.
        if self.current_frame.is_none()
            && matches!(
                animation.state,
                AnimationState::Static(_)
                    | AnimationState::Animated { .. }
                    | AnimationState::Error(_)
            )
        {
            self.load_animation(animation)?;
            // Reset frame_elapsed to avoid jumping ahead
            self.frame_elapsed = Duration::ZERO;
            return Ok(false);
        }

        self.elapsed_time += dt;
        self.frame_elapsed += dt;

        // Determine the duration of the current frame
        let current_frame_dur = match &self.current_frame {
            Some(f) => {
                if f.duration.is_zero() {
                    Duration::from_millis(100)
                } else {
                    f.duration
                }
            }
            None => Duration::from_millis(100),
        };

        // Advance frames if needed
        if self.frame_elapsed >= current_frame_dur {
            // We need to move to the next frame
            if let AnimationState::Animated {
                bytes,
                format,
                first_frame,
            } = &animation.state
            {
                // Keep consuming frames as long as we are overdue
                while self.frame_elapsed >= current_frame_dur {
                    self.frame_elapsed -= current_frame_dur;

                    let mut reached_end = false;
                    let mut tick_err = None;

                    if let Some(decoder) = &mut self.active_decoder {
                        match decoder.next() {
                            Some(Ok(image_frame)) => {
                                let (num, denom) = image_frame.delay().numer_denom_ms();
                                let duration = if denom == 0 {
                                    Duration::from_millis(100)
                                } else {
                                    Duration::from_millis((num / denom) as u64)
                                };
                                self.current_frame = Some(Frame {
                                    data: image_frame.into_buffer(),
                                    duration,
                                });
                                self.current_frame_idx += 1;
                            }
                            Some(Err(e)) => {
                                tick_err = Some(e.to_string());
                            }
                            None => {
                                reached_end = true;
                            }
                        }
                    } else {
                        reached_end = true;
                    }

                    if let Some(err) = tick_err {
                        if self.stored_state.is_none() {
                            self.stored_state = Some(self.state);
                        }
                        self.state = PlaybackState::Paused;
                        return Err(err);
                    }

                    if reached_end {
                        self.loops_completed += 1;

                        // Check if we should move to the next animation BEFORE restarting decoder
                        if self.should_advance_animation() {
                            self.current_command_idx += 1;
                            return Ok(true);
                        }

                        // Restart decoder
                        if let Ok(mut decoder) = LazyDecoder::new(bytes.clone(), *format) {
                            // Immediately use the cached first frame instead of decoding it again
                            self.current_frame = Some(first_frame.clone());
                            self.current_frame_idx = 0;
                            let _ = decoder.next(); // Advance iterator past the first frame

                            self.active_decoder = Some(decoder);
                        } else {
                            if self.stored_state.is_none() {
                                self.stored_state = Some(self.state);
                            }
                            self.state = PlaybackState::Paused;
                            return Err("Failed to restart animation loop".to_string());
                        }
                    }
                }
            } else if let AnimationState::Static(_) = &animation.state {
                // For static images, one "loop" is just the duration of the single frame (default 100ms)
                while self.frame_elapsed >= current_frame_dur {
                    self.frame_elapsed -= current_frame_dur;
                    self.loops_completed += 1;
                    if self.should_advance_animation() {
                        self.current_command_idx += 1;
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }

    /// Returns the exact duration to wait until the next frame should be displayed.
    pub fn time_until_next_frame(&self, _animation: &Animation) -> Duration {
        if self.state == PlaybackState::Paused {
            return Duration::from_millis(100);
        }

        if let Some(f) = &self.current_frame {
            let dur = if f.duration.is_zero() {
                Duration::from_millis(100)
            } else {
                f.duration
            };

            if self.frame_elapsed >= dur {
                return Duration::ZERO;
            }
            return dur - self.frame_elapsed;
        }

        Duration::from_millis(100)
    }

    pub fn current_frame_index(&self) -> usize {
        self.current_frame_idx
    }

    pub fn current_frame(&self) -> Option<&Frame> {
        self.current_frame.as_ref()
    }

    pub fn state(&self) -> PlaybackState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Animation, AnimationState, Frame};
    use image::RgbaImage;

    // Helper to test iterator directly instead of building WebP bytes
    fn create_test_animation() -> Animation {
        // We'll just test with Static frames or similar if possible.
        // Actually, since LazyDecoder requires valid image bytes, we can't easily mock `Animated` state
        // without providing a valid gif/webp file.
        // For unit tests, we'll use `Static` to test basic timing logic.
        Animation {
            source_path: std::path::PathBuf::from("test.png"),
            format: image::ImageFormat::Png,
            state: AnimationState::Static(Frame {
                data: RgbaImage::new(1, 1),
                duration: Duration::from_millis(500),
            }),
        }
    }

    #[test]
    fn test_tick_and_advance() {
        let anim = create_test_animation();

        let cmds = vec![TimingCommand {
            loops: Some(2), // 2 loops of 500ms = 1000ms
            time_secs: None,
            infinite: false,
        }];
        let mut manager = SlideshowManager::new(cmds);

        manager.load_animation(&anim).unwrap();

        // Tick 400ms: Still loop 0
        assert!(!manager.tick(Duration::from_millis(400), &anim).unwrap());

        // Tick 200ms (Total 600ms): Loop 1
        assert!(!manager.tick(Duration::from_millis(200), &anim).unwrap());

        // Tick 400ms (Total 1000ms): Should finish and advance
        assert!(manager.tick(Duration::from_millis(400), &anim).unwrap());
    }
}
