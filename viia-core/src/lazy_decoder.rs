use image::{AnimationDecoder, Frame, ImageFormat, ImageResult};
use std::io::Cursor;
use std::sync::Arc;

/// A simple newtype to allow `Arc<Vec<u8>>` to be used with `std::io::Cursor`.
#[derive(Clone)]
pub struct ArcBytes(pub Arc<Vec<u8>>);

impl AsRef<[u8]> for ArcBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// A lazily-evaluating decoder iterator for an animated image.
/// This prevents loading all frames of a large animation into memory at once.
pub struct LazyDecoder {
    iter: Box<dyn Iterator<Item = ImageResult<Frame>>>,
}

impl LazyDecoder {
    pub fn new(
        bytes: Arc<Vec<u8>>,
        format: ImageFormat,
    ) -> Result<Self, crate::image_loader::EngineError> {
        let cursor = Cursor::new(ArcBytes(bytes));

        let iter: Box<dyn Iterator<Item = ImageResult<Frame>>> = match format {
            ImageFormat::Gif => {
                let decoder = image::codecs::gif::GifDecoder::new(cursor)?;
                Box::new(decoder.into_frames())
            }
            ImageFormat::WebP => {
                let decoder = image::codecs::webp::WebPDecoder::new(cursor)?;
                Box::new(decoder.into_frames())
            }
            _ => return Err(crate::image_loader::EngineError::UnsupportedFormat),
        };

        Ok(Self { iter })
    }
}

impl Iterator for LazyDecoder {
    type Item = ImageResult<Frame>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
