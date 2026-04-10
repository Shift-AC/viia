pub mod cache;
pub mod image_loader;
pub mod internal_shell;
pub mod resizer;
pub mod slideshow_parser;
pub mod state_machine;

pub use cache::{CacheKey, FrameCache};
pub use image_loader::{Animation, AnimationState, EngineError, Frame};
pub use internal_shell::{InternalCommand, RuntimeAction, ZoomMode};
pub use resizer::resize_image;
pub use slideshow_parser::{ParserError, TimingCommand, parse_slideshow_spec};
pub use state_machine::{PlaybackState, SlideshowManager};

use std::fs;
use std::path::PathBuf;

pub fn collect_image_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut resolved_paths = Vec::new();

    if paths.len() == 1 && paths[0].is_file() {
        let target_file = &paths[0];
        resolved_paths.push(target_file.clone());

        if let Some(parent) = target_file.parent()
            && let Ok(entries) = fs::read_dir(parent)
        {
            let mut siblings = Vec::new();
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type()
                    && file_type.is_file() {
                        let path = entry.path();
                        if path != *target_file
                            && let Some(ext) = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(|s| s.to_lowercase())
                            && matches!(
                                ext.as_str(),
                                "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp"
                            )
                        {
                            siblings.push(path);
                        }
                    }
            }
            siblings.sort();
            resolved_paths.extend(siblings);
        }
    } else {
        for path in paths {
            if path.is_dir() {
                if let Ok(entries) = fs::read_dir(path) {
                    let mut dir_paths = Vec::new();
                    for entry in entries.flatten() {
                        if let Ok(file_type) = entry.file_type()
                            && file_type.is_file() {
                                let p = entry.path();
                                if let Some(ext) = p
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .map(|s| s.to_lowercase())
                                    && matches!(
                                        ext.as_str(),
                                        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp"
                                    )
                                {
                                    dir_paths.push(p);
                                }
                            }
                    }
                    dir_paths.sort();
                    resolved_paths.extend(dir_paths);
                }
            } else {
                resolved_paths.push(path);
            }
        }
    }
    resolved_paths
}

pub fn update_prefetch(animations: &mut [Animation], current_idx: usize, prefetch: usize) {
    let len = animations.len();
    if len == 0 {
        return;
    }
    
    // First, unparse everything outside the window
    for (i, anim) in animations.iter_mut().enumerate() {
        let is_in_window = if prefetch >= len {
            true
        } else {
            let dist = if i >= current_idx {
                i - current_idx
            } else {
                len - current_idx + i
            };
            dist <= prefetch
        };
        
        if !is_in_window {
            anim.unparse();
        }
    }
    
    // Then, parse the first unparsed image in the window
    // We prioritize images closer to current_idx
    for offset in 0..=prefetch {
        if offset >= len {
            break;
        }
        let idx = (current_idx + offset) % len;
        if matches!(animations[idx].state, AnimationState::Skimmed) {
            let _ = animations[idx].parse();
            // Only parse one per call to avoid blocking the main thread too long
            break;
        }
    }
}
