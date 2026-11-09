use std::{io, marker::PhantomData, rc::Rc};
use tracing::{info, warn};
use viia_core::{Animation, AnimationManager, AnimationState, PlaybackState};

/// Custom slideshow playback needs an awake display even when showing a static image.
/// The default infinite image loop, paused playback, and unloaded/error images do not.
pub(super) fn should_keep_awake(manager: &AnimationManager, animation: Option<&Animation>) -> bool {
    manager.spec.is_some()
        && manager.state() == PlaybackState::Playing
        && manager.current_frame().is_some()
        && animation.is_some_and(|animation| {
            matches!(
                animation.state,
                AnimationState::Static { .. } | AnimationState::Animated { .. }
            )
        })
}

/// Owns the playback thread's execution-state request. It must never move to another
/// thread: Windows clears the request on that thread's exit, and resets are thread-local.
pub(super) struct DisplayInhibitor {
    requested: bool,
    reset_needed: bool,
    set_execution_state: fn(bool) -> io::Result<()>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl DisplayInhibitor {
    pub(super) fn new() -> Self {
        Self {
            requested: false,
            reset_needed: false,
            set_execution_state,
            _thread_bound: PhantomData,
        }
    }

    pub(super) fn update(&mut self, keep_awake: bool) {
        if self.requested == keep_awake {
            return;
        }
        // Attempt once per transition so an OS failure cannot flood the frame loop's logs.
        // A subsequent pause/resume retries the request.
        self.requested = keep_awake;
        self.reset_needed |= keep_awake;
        match (self.set_execution_state)(keep_awake) {
            Ok(()) => {
                self.reset_needed = keep_awake;
                info!(keep_awake, "Updated GUI display and system idle inhibition");
            }
            Err(error) => warn!(
                keep_awake,
                %error,
                "Failed to update GUI display and system idle inhibition"
            ),
        }
    }
}

impl Drop for DisplayInhibitor {
    fn drop(&mut self) {
        // Also reset after failed activation or reset attempts, in case the OS
        // state is uncertain. Thread exit is Windows' final cleanup fallback.
        if self.reset_needed {
            self.requested = true;
            self.update(false);
        }
    }
}

#[cfg(windows)]
fn set_execution_state(keep_awake: bool) -> io::Result<()> {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    let flags = if keep_awake {
        ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    // SAFETY: This API takes only execution-state flags, with no pointers. The guard
    // keeps all calls on the owning playback thread and resets its request on drop.
    if unsafe { SetThreadExecutionState(flags) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn set_execution_state(_keep_awake: bool) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;
    use std::{sync::Mutex, time::Duration};
    use viia_core::{Frame, MediaUrl, TimingCommand};

    fn static_animation() -> Animation {
        Animation {
            source: MediaUrl::parse_url("file:///test.png").unwrap(),
            format: image::ImageFormat::Png,
            state: AnimationState::Static {
                frame: Frame {
                    data: RgbaImage::new(1, 1),
                    duration: Duration::from_millis(100),
                },
                format: image::ImageFormat::Png,
            },
        }
    }

    fn default_manager(animation: &Animation) -> AnimationManager {
        let mut manager = AnimationManager::new(vec![TimingCommand {
            loops: None,
            time_secs: None,
            infinite: true,
        }]);
        manager.load_animation(animation).unwrap();
        manager
    }

    #[test]
    fn custom_playback_inhibits_until_paused_or_reset_to_default() {
        let animation = static_animation();
        let mut manager = default_manager(&animation);
        assert!(!should_keep_awake(&manager, Some(&animation)));

        manager
            .set_commands(
                vec![TimingCommand {
                    loops: None,
                    time_secs: Some(5.0),
                    infinite: false,
                }],
                &animation,
                "T5".into(),
            )
            .unwrap();
        assert!(should_keep_awake(&manager, Some(&animation)));
        manager.toggle_pause();
        assert!(!should_keep_awake(&manager, Some(&animation)));
        manager.toggle_pause();
        assert!(should_keep_awake(&manager, Some(&animation)));
        assert!(!should_keep_awake(&manager, None));

        manager = default_manager(&animation);
        assert!(!should_keep_awake(&manager, Some(&animation)));
    }

    #[test]
    fn unloaded_and_error_images_do_not_inhibit() {
        let mut animation = static_animation();
        let mut manager = default_manager(&animation);
        manager.spec = Some("T5".into());
        animation.state = AnimationState::Skimmed;
        assert!(!should_keep_awake(&manager, Some(&animation)));
        animation.state = AnimationState::Error("decode failed".into());
        assert!(!should_keep_awake(&manager, Some(&animation)));
        assert!(manager.load_animation(&animation).is_err());
        assert_eq!(manager.state(), PlaybackState::Paused);
        assert!(!should_keep_awake(&manager, Some(&animation)));
    }

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static CALLS: Mutex<Vec<bool>> = Mutex::new(Vec::new());

    fn record_request(keep_awake: bool) -> io::Result<()> {
        CALLS.lock().unwrap().push(keep_awake);
        Ok(())
    }

    fn assert_requests(expected: &[bool]) {
        assert_eq!(*CALLS.lock().unwrap(), expected);
    }

    fn fail_request(keep_awake: bool) -> io::Result<()> {
        record_request(keep_awake)?;
        Err(io::Error::other("simulated OS failure"))
    }

    fn test_inhibitor(setter: fn(bool) -> io::Result<()>) -> DisplayInhibitor {
        CALLS.lock().unwrap().clear();
        DisplayInhibitor {
            requested: false,
            reset_needed: false,
            set_execution_state: setter,
            _thread_bound: PhantomData,
        }
    }

    #[test]
    fn repeated_frames_do_not_repeat_requests_and_drop_releases() {
        let _lock = TEST_LOCK.lock().unwrap();
        let mut inhibitor = test_inhibitor(record_request);
        inhibitor.update(false);
        inhibitor.update(true);
        inhibitor.update(true);
        inhibitor.update(false);
        inhibitor.update(false);
        inhibitor.update(true);
        drop(inhibitor);
        assert_requests(&[true, false, true, false]);
    }

    #[test]
    fn failures_are_not_repeated_each_frame_and_are_reset_on_drop() {
        let _lock = TEST_LOCK.lock().unwrap();
        let mut inhibitor = test_inhibitor(fail_request);
        inhibitor.update(true);
        inhibitor.update(true);
        inhibitor.update(false);
        inhibitor.update(true);
        drop(inhibitor);
        assert_requests(&[true, false, true, false]);
    }

    #[test]
    fn failed_reset_is_retried_on_drop() {
        let _lock = TEST_LOCK.lock().unwrap();
        let mut inhibitor = test_inhibitor(fail_request);
        inhibitor.update(true);
        inhibitor.update(false);
        drop(inhibitor);
        assert_requests(&[true, false, false]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_request_sets_display_and_system_flags_and_drop_clears_them() {
        use windows_sys::Win32::System::Power::{
            ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
        };

        let flags = ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED;
        let mut inhibitor = DisplayInhibitor::new();
        inhibitor.update(true);
        // SAFETY: Only valid flags are passed on the guard's owning test thread.
        // Reasserting the flags returns the previous state without clearing the request.
        let previous = unsafe { SetThreadExecutionState(flags) };
        drop(inhibitor);
        assert_eq!(previous & flags, flags);
        // SAFETY: Reset the same test thread and inspect the state left by Drop.
        let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        assert_ne!(previous, 0);
        assert_eq!(previous & (ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED), 0);
    }
}
