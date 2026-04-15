use crate::MediaUrl;
use crate::cache::{CacheKey, FrameCache};
use image::RgbaImage;
use std::sync::Arc;

#[test]
fn test_cache_insert_and_get() {
    let cache = FrameCache::new(2);

    let img = Arc::new(RgbaImage::new(10, 10));
    let key = CacheKey {
        source: MediaUrl::parse_url("file:///test.png").unwrap(),
        frame_index: 0,
        target_width: 10,
        target_height: 10,
    };

    cache.insert(key.clone(), img.clone());

    let retrieved = cache.get(&key).expect("Should be in cache");
    assert_eq!(retrieved.width(), 10);
}

#[test]
fn test_get_or_resize() {
    let cache = FrameCache::new(5);
    let original = RgbaImage::new(100, 200);

    let key = CacheKey {
        source: MediaUrl::parse_url("file:///test.png").unwrap(),
        frame_index: 0,
        target_width: 50,
        target_height: 50,
    };

    // First call should calculate and cache
    let resized1 = cache
        .get_or_resize(key.clone(), &original)
        .expect("Should resize");
    assert_eq!(resized1.width(), 25);
    assert_eq!(resized1.height(), 50);

    // Second call should fetch from cache directly
    let resized2 = cache.get(&key).expect("Should be in cache");
    assert_eq!(resized2.width(), 25);
}
