use crate::Animation;
use crate::slideshow_parser::TimingCommand;
use std::time::{Duration, Instant};
use tracing::{debug, info, trace};

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

    /// Total duration of a single loop of the current animation
    current_loop_duration: Duration,
    /// Effective duration this animation should play (longest of L or T)
    effective_duration: Duration,

    /// State tracking
    state: PlaybackState,
    start_time: Option<Instant>,
    elapsed_time: Duration,

    /// Current frame within the current animation
    current_frame_idx: usize,
    frame_elapsed: Duration,
}

impl SlideshowManager {
    pub fn new(commands: Vec<TimingCommand>) -> Self {
        Self {
            commands,
            current_command_idx: 0,
            current_loop_duration: Duration::ZERO,
            effective_duration: Duration::ZERO,
            state: PlaybackState::Playing,
            start_time: None,
            elapsed_time: Duration::ZERO,
            current_frame_idx: 0,
            frame_elapsed: Duration::ZERO,
        }
    }

    /// Sets a new set of timing commands and resets the current state
    pub fn set_commands(&mut self, commands: Vec<TimingCommand>, current_animation: &Animation) {
        info!("Setting new commands: {:?}", commands);
        self.commands = commands;
        self.current_command_idx = 0;
        self.load_animation(current_animation);
    }

    /// Initializes the manager for a new animation
    pub fn load_animation(&mut self, animation: &Animation) {
        let loop_dur = match &animation.state {
            crate::AnimationState::Parsed(frames) => frames.iter().map(|f| f.duration).sum(),
            crate::AnimationState::Skimmed => Duration::from_millis(100), // Default for unparsed
            crate::AnimationState::Error(_) => Duration::from_millis(100), // Default for error
        };

        self.current_loop_duration = loop_dur;

        let cmd = self.get_current_command().clone();
        self.effective_duration = cmd.calculate_effective_duration(loop_dur);

        info!("Loaded animation: {:?}", animation.source_path);
        info!("Current command: {:?}", cmd);
        debug!(
            "Loop duration: {:?}, Effective duration: {:?}",
            loop_dur, self.effective_duration
        );

        // Issue 3 fix: When loading a new animation, if we are playing, update start_time to now.
        // Otherwise, leave it as None (paused). We must also reset frame_elapsed and elapsed_time.
        if self.state == PlaybackState::Playing {
            self.start_time = Some(Instant::now());
        } else {
            self.start_time = None;
        }
        self.elapsed_time = Duration::ZERO;
        self.current_frame_idx = 0;
        self.frame_elapsed = Duration::ZERO;
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

    /// Ticks the state machine by `dt`.
    /// Returns `true` if the manager should advance to the next image in the list.
    pub fn tick(&mut self, dt: Duration, animation: &Animation) -> bool {
        if self.state == PlaybackState::Paused {
            return false;
        }

        self.elapsed_time += dt;
        self.frame_elapsed += dt;

        // Check if we need to advance frame
        if let crate::AnimationState::Parsed(frames) = &animation.state
            && !frames.is_empty()
        {
            if self.current_frame_idx >= frames.len() {
                self.current_frame_idx = 0;
            }
            loop {
                let mut current_frame_dur = frames[self.current_frame_idx].duration;
                if current_frame_dur.is_zero() {
                    current_frame_dur = Duration::from_millis(100);
                }
                if self.frame_elapsed < current_frame_dur {
                    break;
                }
                self.frame_elapsed -= current_frame_dur;
                let old_frame = self.current_frame_idx;
                self.current_frame_idx = (self.current_frame_idx + 1) % frames.len();
                trace!(
                    "Advanced to frame {} (from {})",
                    self.current_frame_idx, old_frame
                );
            }
        }

        // Check if the entire animation's effective duration has expired
        if self.elapsed_time >= self.effective_duration {
            if self.current_loop_duration.is_zero() {
                self.current_command_idx += 1;
                debug!(
                    "Effective duration {:?} reached, moving to next command (idx: {})",
                    self.effective_duration, self.current_command_idx
                );
                return true;
            }

            // We only advance on a loop boundary
            // We check this by seeing if the elapsed time is a multiple of the loop duration
            let remainder =
                self.elapsed_time.as_secs_f64() % self.current_loop_duration.as_secs_f64();

            // Allow a small epsilon for floating point / timing inaccuracies (e.g. 10ms)
            if remainder < 0.01 || (self.current_loop_duration.as_secs_f64() - remainder) < 0.01 {
                self.current_command_idx += 1;
                debug!(
                    "Effective duration {:?} reached at loop boundary, moving to next command (idx: {})",
                    self.effective_duration, self.current_command_idx
                );
                return true;
            }
        }

        false
    }

    /// Returns the exact duration to wait until the next frame should be displayed.
    /// If the result is zero, the next frame is already overdue and should be displayed immediately.
    pub fn time_until_next_frame(&self, animation: &Animation) -> Duration {
        if self.state == PlaybackState::Paused {
            // Arbitrary sleep time when paused (e.g. 100ms) to avoid busy looping
            return Duration::from_millis(100);
        }

        if let crate::AnimationState::Parsed(frames) = &animation.state
            && !frames.is_empty()
        {
            let current_frame_dur = if frames[self.current_frame_idx].duration.is_zero() {
                Duration::from_millis(100)
            } else {
                frames[self.current_frame_idx].duration
            };

            if self.frame_elapsed >= current_frame_dur {
                return Duration::ZERO; // Overdue
            }
            return current_frame_dur - self.frame_elapsed;
        }

        // Default sleep if no valid frames
        Duration::from_millis(100)
    }

    pub fn current_frame_index(&self) -> usize {
        self.current_frame_idx
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

    fn create_test_animation(frame_durs: Vec<u64>) -> Animation {
        let frames = frame_durs
            .into_iter()
            .map(|ms| Frame {
                data: RgbaImage::new(1, 1),
                duration: Duration::from_millis(ms),
            })
            .collect();

        Animation {
            source_path: std::path::PathBuf::from("test.gif"),
            format: image::ImageFormat::Gif,
            state: AnimationState::Parsed(frames),
        }
    }

    #[test]
    fn test_tick_and_advance() {
        // Animation is 2 frames: 100ms and 200ms (Loop = 300ms)
        let anim = create_test_animation(vec![100, 200]);

        // Command: Loop 2 times (Total = 600ms)
        let cmds = vec![TimingCommand {
            loops: Some(2),
            time_secs: None,
            infinite: false,
        }];
        let mut manager = SlideshowManager::new(cmds);

        manager.load_animation(&anim);
        assert_eq!(manager.effective_duration, Duration::from_millis(600));

        // Tick 50ms: Still frame 0
        assert!(!manager.tick(Duration::from_millis(50), &anim));
        assert_eq!(manager.current_frame_index(), 0);

        // Tick 60ms (Total 110ms): Advanced to frame 1
        assert!(!manager.tick(Duration::from_millis(60), &anim));
        assert_eq!(manager.current_frame_index(), 1);

        // Tick 490ms (Total 600ms): Should finish and advance to next image
        assert!(manager.tick(Duration::from_millis(490), &anim));
    }

    #[test]
    fn test_tick_waits_for_loop_boundary() {
        // Animation is 1 frame: 500ms
        let anim = create_test_animation(vec![500]);

        // Command: Time 600ms. Since loop is 500ms, it should wait until 1000ms (end of 2nd loop)
        let cmds = vec![TimingCommand {
            loops: None,
            time_secs: Some(0.6),
            infinite: false,
        }];
        let mut manager = SlideshowManager::new(cmds);

        manager.load_animation(&anim);

        // Use tolerance to avoid floating point mismatch
        let diff = if manager.effective_duration > Duration::from_millis(600) {
            manager.effective_duration - Duration::from_millis(600)
        } else {
            Duration::from_millis(600) - manager.effective_duration
        };
        assert!(diff.as_millis() <= 1);

        // Tick 600ms: Duration reached, but not at loop boundary (remainder = 100ms)
        assert!(!manager.tick(Duration::from_millis(600), &anim));

        // Tick 400ms (Total 1000ms): Duration reached AND at loop boundary
        assert!(manager.tick(Duration::from_millis(400), &anim));
    }

    #[test]
    fn test_tick_with_computation_delay() {
        // Animation is 3 frames, 33ms each (~30fps)
        let anim = create_test_animation(vec![33, 33, 33]);

        let cmds = vec![TimingCommand {
            loops: Some(1),
            time_secs: None,
            infinite: false,
        }];
        let mut manager = SlideshowManager::new(cmds);

        manager.load_animation(&anim);

        // Frame 0 initially
        assert_eq!(manager.current_frame_index(), 0);
        assert_eq!(
            manager.time_until_next_frame(&anim),
            Duration::from_millis(33)
        );

        // Simulate a scenario where resizing takes 20ms.
        // We only tick 20ms. The frame shouldn't advance yet.
        manager.tick(Duration::from_millis(20), &anim);
        assert_eq!(manager.current_frame_index(), 0);

        // Time remaining should be exactly 13ms. This handles the rendering/processing cost.
        assert_eq!(
            manager.time_until_next_frame(&anim),
            Duration::from_millis(13)
        );

        // If the render takes 50ms (overdue), it should instantly advance
        // It has passed the first 33ms frame, and consumed 17ms of the second 33ms frame.
        // So we are now on frame 1, and 17ms into it.
        manager.tick(Duration::from_millis(30), &anim); // Total elapsed = 50ms
        assert_eq!(manager.current_frame_index(), 1);

        // 50ms elapsed. Frame 0 took 33ms. We are 17ms into Frame 1 (which lasts 33ms).
        // Time remaining until Frame 2 should be 16ms.
        assert_eq!(
            manager.time_until_next_frame(&anim),
            Duration::from_millis(16)
        );
    }
}
