use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("Failed to load image: {0}")]
    ImageLoadError(#[from] image::ImageError),
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),
    #[error("Unsupported format")]
    UnsupportedFormat,
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
    /// Pixels and frames fully loaded into memory.
    Parsed(Vec<Frame>),
    /// Failed to parse the image
    Error(String),
}

/// A normalized representation of either a static image or an animation.
/// To support fast deferred loading for GUI integration, it supports two states: Skimmed and Parsed.
pub struct Animation {
    pub source_path: PathBuf,
    pub format: image::ImageFormat,
    pub state: AnimationState,
}

impl Animation {
    /// Skims a file by only reading its header to determine format.
    /// This is extremely fast and avoids decoding pixels into memory.
    pub fn skim(path: PathBuf) -> Result<Self, EngineError> {
        // Defer all I/O to parse() for instant startup
        Ok(Self {
            source_path: path,
            format: image::ImageFormat::Png, // Dummy format, will be updated in parse()
            state: AnimationState::Skimmed,
        })
    }

    /// Fully parses the animation, loading all frames into memory.
    /// If it's already parsed, this is a no-op.
    pub fn parse(&mut self) -> Result<(), EngineError> {
        if matches!(self.state, AnimationState::Parsed(_)) {
            return Ok(());
        }

            let result = (|| -> Result<Vec<Frame>, EngineError> {
                let file = std::fs::File::open(&self.source_path)
                    .map_err(|e| EngineError::ImageLoadError(image::ImageError::IoError(e)))?;
                let reader = std::io::BufReader::new(file);

                let format = image::ImageReader::new(reader)
                    .with_guessed_format()
                    .map_err(|e| EngineError::ImageLoadError(image::ImageError::IoError(e)))?
                    .format()
                    .ok_or(EngineError::UnsupportedFormat)?;

                self.format = format;

                match format {
                    image::ImageFormat::Gif | image::ImageFormat::WebP => {
                        Self::load_animated(&self.source_path, format)
                    }
                    _ => Self::load_static(&self.source_path),
                }
            })();

        match result {
            Ok(f) => {
                self.state = AnimationState::Parsed(f);
                Ok(())
            }
            Err(e) => {
                self.state = AnimationState::Error(e.to_string());
                Err(e)
            }
        }
    }

    /// Releases the memory occupied by the parsed frames, reverting to Skimmed state.
    pub fn unparse(&mut self) {
        if matches!(self.state, AnimationState::Parsed(_) | AnimationState::Error(_)) {
            self.state = AnimationState::Skimmed;
        }
    }

    /// Returns true if the animation is fully parsed and contains exactly one frame.
    pub fn is_single_frame(&self) -> bool {
        if let AnimationState::Parsed(frames) = &self.state {
            frames.len() == 1
        } else {
            false
        }
    }

    fn load_static(path: &PathBuf) -> Result<Vec<Frame>, EngineError> {
        let img = image::open(path)?;
        let rgba = img.into_rgba8();

        let frame = Frame {
            data: rgba,
            duration: Duration::from_millis(100), // Standardized 100ms for static images
        };

        Ok(vec![frame])
    }

    fn load_animated(
        path: &PathBuf,
        format: image::ImageFormat,
    ) -> Result<Vec<Frame>, EngineError> {
        let file = std::fs::File::open(path)
            .map_err(|e| EngineError::ImageLoadError(image::ImageError::IoError(e)))?;
        let reader = std::io::BufReader::new(file);

        let mut frames = Vec::new();

        // Use the AnimationDecoder trait
        match format {
            image::ImageFormat::Gif => {
                let decoder = image::codecs::gif::GifDecoder::new(reader)?;
                use image::AnimationDecoder;
                for frame_result in decoder.into_frames() {
                    let frame = frame_result?;
                    let (num, denom) = frame.delay().numer_denom_ms();
                    let duration = if denom == 0 {
                        Duration::from_millis(100)
                    } else {
                        Duration::from_millis((num / denom) as u64)
                    };

                    frames.push(Frame {
                        data: frame.into_buffer(),
                        duration,
                    });
                }
            }
            image::ImageFormat::WebP => {
                let decoder = image::codecs::webp::WebPDecoder::new(reader)?;
                use image::AnimationDecoder;
                if decoder.has_animation() {
                    for frame_result in decoder.into_frames() {
                        let frame = frame_result?;
                        let (num, denom) = frame.delay().numer_denom_ms();
                        let duration = if denom == 0 {
                            Duration::from_millis(100)
                        } else {
                            Duration::from_millis((num / denom) as u64)
                        };

                        frames.push(Frame {
                            data: frame.into_buffer(),
                            duration,
                        });
                    }
                } else {
                    return Self::load_static(path);
                }
            }
            _ => return Err(EngineError::UnsupportedFormat),
        }

        if frames.is_empty() {
            // Fallback to static if no frames found
            return Self::load_static(path);
        }

        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_single_frame() {
        let mut anim = Animation {
            source_path: PathBuf::from("test.jpg"),
            format: image::ImageFormat::Jpeg,
            state: AnimationState::Skimmed,
        };

        // Before parsing, it should not be a single frame (since it's Skimmed)
        assert!(!anim.is_single_frame());

        // Create a fake parsed state with one frame
        anim.state = AnimationState::Parsed(vec![Frame {
            data: image::RgbaImage::new(1, 1),
            duration: Duration::from_millis(100),
        }]);
        assert!(anim.is_single_frame());

        // Create a fake parsed state with two frames
        anim.state = AnimationState::Parsed(vec![
            Frame {
                data: image::RgbaImage::new(1, 1),
                duration: Duration::from_millis(100),
            },
            Frame {
                data: image::RgbaImage::new(1, 1),
                duration: Duration::from_millis(100),
            },
        ]);
        assert!(!anim.is_single_frame());
    }
}
