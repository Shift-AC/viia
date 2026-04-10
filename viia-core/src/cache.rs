use crate::resizer::resize_image;
use image::RgbaImage;
use moka::sync::Cache;
use std::path::PathBuf;
use std::sync::Arc;

/// Cache key for resized frames
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub path: PathBuf,
    pub frame_index: usize,
    pub target_width: u32,
    pub target_height: u32,
}

/// A highly concurrent thread-safe cache for decoded and resized frames.
/// Unlike `lru` which requires a mutable borrow on `get` and introduces lock contention,
/// `moka` allows concurrent lock-free reads while still providing LRU semantics via a background thread.
pub struct FrameCache {
    cache: Cache<CacheKey, Arc<RgbaImage>>,
}

impl FrameCache {
    /// Creates a new cache with the specified capacity (number of frames)
    pub fn new(capacity: u64) -> Self {
        Self {
            cache: Cache::new(capacity),
        }
    }

    /// Gets a frame from the cache if it exists
    pub fn get(&self, key: &CacheKey) -> Option<Arc<RgbaImage>> {
        self.cache.get(key)
    }

    /// Inserts a frame into the cache
    pub fn insert(&self, key: CacheKey, frame: Arc<RgbaImage>) {
        self.cache.insert(key, frame);
    }

    /// Helper to get a resized frame, calculating and caching it if missing
    pub fn get_or_resize(
        &self,
        key: CacheKey,
        original_frame: &RgbaImage,
    ) -> Option<Arc<RgbaImage>> {
        if let Some(cached) = self.get(&key) {
            return Some(cached);
        }

        let resized = resize_image(original_frame, key.target_width, key.target_height)?;
        let arc_resized = Arc::new(resized);
        self.insert(key, arc_resized.clone());

        Some(arc_resized)
    }

    /// Clears the cache
    pub fn clear(&self) {
        self.cache.invalidate_all();
    }
}

// Default implementation with a sensible capacity (e.g., 500 frames)
impl Default for FrameCache {
    fn default() -> Self {
        Self::new(500)
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
