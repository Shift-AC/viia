use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::MediaUrl;
use crate::source_access::SourceAccess;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("Failed to load image: {0}")]
    ImageLoadError(#[from] image::ImageError),
    #[error("Source not found: {0}")]
    SourceNotFound(MediaUrl),
    #[error("Unsupported format")]
    UnsupportedFormat,
    #[error("Unsupported scheme: {0}")]
    UnsupportedScheme(String),
    #[error("Source error: {0}")]
    SourceError(String),
}

/// A single frame of an animation
#[derive(Clone)]
pub struct Frame {
    /// RGBA8 image data
    pub data: image::RgbaImage,
    /// Duration this frame should be displayed
    pub duration: Duration,
}

/// The state of an animation
#[derive(Clone)]
pub enum AnimationState {
    /// Barely checked existence and header. No pixels loaded.
    Skimmed,
    /// Currently parsing the image in a background thread.
    /// The AtomicBool acts as a cancellation token.
    Parsing(ParseState, Arc<AtomicBool>),
    /// A single-frame static image fully parsed into memory.
    Static {
        frame: Frame,
        format: image::ImageFormat,
    },
    /// An animated image file, keeping compressed bytes in memory for lazy decoding.
    Animated {
        bytes: Arc<Vec<u8>>,
        format: image::ImageFormat,
        first_frame: Frame,
    },
    /// Failed to parse the image
    Error(String),
}

type ParseState = Arc<(
    std::sync::Mutex<Option<Result<AnimationState, String>>>,
    std::sync::Condvar,
)>;

/// A normalized representation of either a static image or an animation.
/// To support fast deferred loading for GUI integration, it supports two states: Skimmed and Parsed.
pub struct Animation {
    pub source: MediaUrl,
    pub format: image::ImageFormat,
    pub state: AnimationState,
}

impl Animation {
    /// Skims a file by only reading its header to determine format.
    /// This is extremely fast and avoids decoding pixels into memory.
    pub fn skim(source: MediaUrl) -> Result<Self, EngineError> {
        // Defer all I/O to parse() for instant startup
        Ok(Self {
            source,
            format: image::ImageFormat::Png, // Dummy format, will be updated in parse()
            state: AnimationState::Skimmed,
        })
    }

    /// Fully parses the animation, loading all frames into memory.
    /// If it's already parsed, this is a no-op.
    pub fn parse(&mut self) -> Result<(), EngineError> {
        if matches!(
            self.state,
            AnimationState::Static { .. }
                | AnimationState::Animated { .. }
                | AnimationState::Parsing(_, _)
                | AnimationState::Error(_)
        ) {
            return Ok(());
        }

        let result_arc = Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
        let result_arc_clone = Arc::clone(&result_arc);

        let cancel_token = Arc::new(AtomicBool::new(false));
        let cancel_token_clone = Arc::clone(&cancel_token);

        let source = self.source.clone();

        std::thread::spawn(move || {
            let result = (|| -> Result<AnimationState, EngineError> {
                let source_data = source_access().read(&source)?;

                if cancel_token_clone.load(Ordering::Relaxed) {
                    return Err(EngineError::UnsupportedFormat); // Treat cancellation as a generic early exit
                }

                // Check format first
                let format = match &source_data {
                    crate::source_access::SourceData::Bytes(bytes) => {
                        let cursor = std::io::Cursor::new(bytes.as_ref());
                        image::ImageReader::new(cursor)
                            .with_guessed_format()
                            .map_err(|e| EngineError::ImageLoadError(image::ImageError::IoError(e)))?
                            .format()
                            .ok_or(EngineError::UnsupportedFormat)?
                    }
                    crate::source_access::SourceData::LocalFile(path) => {
                        let file = std::fs::File::open(path)
                            .map_err(|e| EngineError::ImageLoadError(image::ImageError::IoError(e)))?;
                        let reader = std::io::BufReader::new(file);
                        image::ImageReader::new(reader)
                            .with_guessed_format()
                            .map_err(|e| EngineError::ImageLoadError(image::ImageError::IoError(e)))?
                            .format()
                            .ok_or(EngineError::UnsupportedFormat)?
                    }
                };

                if cancel_token_clone.load(Ordering::Relaxed) {
                    return Err(EngineError::UnsupportedFormat);
                }

                match format {
                    image::ImageFormat::Gif | image::ImageFormat::WebP => {
                        // Check if it's actually an animation
                        let is_animated = match format {
                            image::ImageFormat::WebP => {
                                match &source_data {
                                    crate::source_access::SourceData::Bytes(bytes) => {
                                        let peek_cursor = std::io::Cursor::new(bytes.as_ref());
                                        image::codecs::webp::WebPDecoder::new(peek_cursor)
                                            .map(|d| d.has_animation())
                                            .unwrap_or(false)
                                    }
                                    crate::source_access::SourceData::LocalFile(path) => {
                                        let peek_file = std::fs::File::open(path).unwrap();
                                        let peek_reader = std::io::BufReader::new(peek_file);
                                        image::codecs::webp::WebPDecoder::new(peek_reader)
                                            .map(|d| d.has_animation())
                                            .unwrap_or(false)
                                    }
                                }
                            }
                            _ => true, // Assuming Gif is usually animated, or handle accordingly
                        };

                        if cancel_token_clone.load(Ordering::Relaxed) {
                            return Err(EngineError::UnsupportedFormat);
                        }

                        if !is_animated {
                            let frames = match &source_data {
                                crate::source_access::SourceData::Bytes(bytes) => {
                                    Self::load_static_from_memory(bytes.as_ref())?
                                }
                                crate::source_access::SourceData::LocalFile(path) => {
                                    Self::load_static_from_path(path)?
                                }
                            };
                            let first_frame = frames.into_iter().next().unwrap();
                            return Ok(AnimationState::Static {
                                frame: first_frame,
                                format,
                            });
                        }

                        if cancel_token_clone.load(Ordering::Relaxed) {
                            return Err(EngineError::UnsupportedFormat);
                        }

                        let bytes = source_data.as_bytes()?;

                        // Do a quick validation check and cache the first frame
                        let mut decoder =
                            crate::lazy_decoder::LazyDecoder::new(bytes.clone(), format)?;
                        let first_frame = match decoder.next() {
                            Some(Ok(image_frame)) => {
                                let (num, denom) = image_frame.delay().numer_denom_ms();
                                let duration = if denom == 0 {
                                    Duration::from_millis(100)
                                } else {
                                    Duration::from_millis((num / denom) as u64)
                                };
                                Frame {
                                    data: image_frame.into_buffer(),
                                    duration,
                                }
                            }
                            Some(Err(e)) => return Err(EngineError::ImageLoadError(e)),
                            None => return Err(EngineError::UnsupportedFormat),
                        };

                        Ok(AnimationState::Animated {
                            bytes,
                            format,
                            first_frame,
                        })
                    }
                    _ => {
                        let frames = match &source_data {
                            crate::source_access::SourceData::Bytes(bytes) => {
                                Self::load_static_from_memory(bytes.as_ref())?
                            }
                            crate::source_access::SourceData::LocalFile(path) => {
                                Self::load_static_from_path(path)?
                            }
                        };
                        let first_frame = frames.into_iter().next().unwrap();
                        Ok(AnimationState::Static {
                            frame: first_frame,
                            format,
                        })
                    }
                }
            })();

            if !cancel_token_clone.load(Ordering::Relaxed) {
                let final_state = match result {
                    Ok(state) => Ok(state),
                    Err(e) => Err(e.to_string()),
                };

                let (lock, cvar) = &*result_arc_clone;
                if let Ok(mut lock) = lock.lock() {
                    *lock = Some(final_state);
                    cvar.notify_all();
                }
            }
        });

        self.state = AnimationState::Parsing(result_arc, cancel_token);
        Ok(())
    }

    /// Releases the memory occupied by the parsed frames, reverting to Skimmed state.
    pub fn unparse(&mut self) {
        if let AnimationState::Parsing(_, cancel_token) = &self.state {
            cancel_token.store(true, Ordering::Relaxed);
            self.state = AnimationState::Skimmed;
        } else if matches!(
            self.state,
            AnimationState::Static { .. }
                | AnimationState::Animated { .. }
                | AnimationState::Error(_)
        ) {
            self.state = AnimationState::Skimmed;
        }
    }

    /// Polls the parsing state to see if it has finished, and updates `self.state` accordingly.
    /// Returns true if the state was updated from Parsing to something else.
    pub fn poll_parse(&mut self) -> bool {
        let mut new_state = None;
        if let AnimationState::Parsing(arc, _) = &self.state {
            let (lock, _) = &**arc;
            if let Ok(mut lock_guard) = lock.try_lock()
                && let Some(result) = lock_guard.take()
            {
                new_state = Some(result);
            }
        }

        if let Some(result) = new_state {
            match result {
                Ok(state) => {
                    if let AnimationState::Animated { format, .. } = &state {
                        self.format = *format;
                    } else if let AnimationState::Static { format, .. } = &state {
                        self.format = *format;
                    }
                    self.state = state;
                }
                Err(e) => self.state = AnimationState::Error(e),
            }
            return true;
        }
        false
    }

    /// Blocks the current thread until the animation is fully parsed or an error occurs.
    /// Returns immediately if already parsed or skimmed.
    pub fn ensure_parsed(&mut self) {
        let mut new_state = None;
        if let AnimationState::Parsing(arc, _) = &self.state {
            let (lock, cvar) = &**arc;
            if let Ok(mut lock_guard) = lock.lock() {
                while lock_guard.is_none() {
                    lock_guard = cvar.wait(lock_guard).unwrap();
                }
                if let Some(result) = lock_guard.take() {
                    new_state = Some(result);
                }
            }
        }

        if let Some(result) = new_state {
            match result {
                Ok(state) => {
                    if let AnimationState::Animated { format, .. } = &state {
                        self.format = *format;
                    } else if let AnimationState::Static { format, .. } = &state {
                        self.format = *format;
                    }
                    self.state = state;
                }
                Err(e) => self.state = AnimationState::Error(e),
            }
        }
    }

    /// Returns true if the animation is fully parsed and contains exactly one frame.
    pub fn is_single_frame(&self) -> bool {
        matches!(&self.state, AnimationState::Static { .. })
    }

    fn load_static_from_memory(bytes: &[u8]) -> Result<Vec<Frame>, EngineError> {
        let img = image::load_from_memory(bytes)?;
        let rgba = img.into_rgba8();

        let frame = Frame {
            data: rgba,
            duration: Duration::from_millis(100), // Standardized 100ms for static images
        };

        Ok(vec![frame])
    }

    fn load_static_from_path(path: &std::path::Path) -> Result<Vec<Frame>, EngineError> {
        let img = image::open(path)?;
        let rgba = img.into_rgba8();

        let frame = Frame {
            data: rgba,
            duration: Duration::from_millis(100), // Standardized 100ms for static images
        };

        Ok(vec![frame])
    }
}

fn source_access() -> &'static SourceAccess {
    static ACCESS: OnceLock<SourceAccess> = OnceLock::new();
    ACCESS.get_or_init(SourceAccess::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_single_frame() {
        let mut anim = Animation {
            source: MediaUrl::parse_url("file:///test.jpg").unwrap(),
            format: image::ImageFormat::Jpeg,
            state: AnimationState::Skimmed,
        };

        // Before parsing, it should not be a single frame (since it's Skimmed)
        assert!(!anim.is_single_frame());

        // Simulate parsed state with 1 frame
        anim.state = AnimationState::Static {
            frame: Frame {
                data: image::RgbaImage::new(1, 1),
                duration: Duration::from_millis(100),
            },
            format: image::ImageFormat::Png,
        };
        assert!(anim.is_single_frame());

        // Simulate parsed state with multiple frames
        anim.state = AnimationState::Animated {
            bytes: std::sync::Arc::new(vec![]),
            format: image::ImageFormat::Gif,
            first_frame: Frame {
                data: image::RgbaImage::new(1, 1),
                duration: Duration::from_millis(100),
            },
        };
        assert!(!anim.is_single_frame());
    }
}
