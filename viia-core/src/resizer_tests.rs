use crate::resizer::resize_image;
use image::{Rgba, RgbaImage};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

#[test]
fn test_resize_aspect_ratio() {
    // Create a 100x200 image (1:2 aspect ratio)
    let mut img = RgbaImage::new(100, 200);
    for pixel in img.pixels_mut() {
        *pixel = Rgba([255, 0, 0, 255]); // Red
    }

    // Target: 50x50 box. Since original is 1:2, it should scale to 25x50 to fit inside 50x50.
    let resized = resize_image(&img, 50, 50).expect("Failed to resize");

    assert_eq!(resized.width(), 25);
    assert_eq!(resized.height(), 50);
}

#[test]
fn test_resize_performance_temp_dir() {
    // Find the project root by going up from the core crate
    let mut temp_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    temp_dir.pop();
    temp_dir.push("temp");

    if !temp_dir.exists() {
        println!(
            "Skipping performance test: 'temp' directory not found at {:?}",
            temp_dir
        );
        return;
    }

    let entries = match fs::read_dir(&temp_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file()
            && let Ok(img) = image::open(&path)
        {
            let rgba = img.into_rgba8();
            let orig_w = rgba.width();
            let orig_h = rgba.height();

            let target_w = orig_w / 2;
            let target_h = orig_h / 2;

            let start = Instant::now();
            let resized = resize_image(&rgba, target_w, target_h);
            let duration = start.elapsed();

            assert!(resized.is_some());
            let result = resized.unwrap();

            println!(
                "Resized {:?} ({}x{} -> {}x{}) in {:?}",
                path.file_name().unwrap(),
                orig_w,
                orig_h,
                result.width(),
                result.height(),
                duration
            );
        }
    }
}
