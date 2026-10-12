pub mod cache;
pub mod image_loader;
pub mod internal_shell;
pub mod lazy_decoder;
pub mod media_url;
pub mod resizer;
pub mod sftp_transport;
pub mod slideshow_parser;
pub mod source_access;
pub mod source_resolver;
pub mod state_machine;

pub use cache::{CacheKey, FrameCache};
pub use image_loader::{Animation, AnimationState, EngineError, Frame};
pub use internal_shell::{
    InfoType, InternalCommand, RuntimeAction, SortMode, ZoomMode, shell_index_to_zero_based,
    zero_based_to_shell_index,
};
pub use media_url::MediaUrl;
pub use resizer::resize_image;
pub use slideshow_parser::{ParserError, TimingCommand, parse_animation_spec};
pub use source_resolver::resolve_media_urls;
pub use state_machine::{AnimationManager, PlaybackState};

use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

pub fn sort_animations(animations: &mut [Animation], modes: &[SortMode]) {
    animations.sort_by(|a, b| {
        for mode in modes {
            let cmp = match mode {
                SortMode::Name => {
                    let name_a = a
                        .source
                        .file_name()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    let name_b = b
                        .source
                        .file_name()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    name_a.cmp(&name_b)
                }
                SortMode::NameDesc => {
                    let name_a = a
                        .source
                        .file_name()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    let name_b = b
                        .source
                        .file_name()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    name_b.cmp(&name_a)
                }
                SortMode::NameNatural => {
                    let name_a = a.source.file_name().unwrap_or_default();
                    let name_b = b.source.file_name().unwrap_or_default();
                    natord::compare_ignore_case(&name_a, &name_b)
                }
                SortMode::NameNaturalDesc => {
                    let name_a = a.source.file_name().unwrap_or_default();
                    let name_b = b.source.file_name().unwrap_or_default();
                    natord::compare_ignore_case(&name_b, &name_a)
                }
                SortMode::Size => {
                    let size_a = get_file_size(&a.source);
                    let size_b = get_file_size(&b.source);
                    size_a.cmp(&size_b)
                }
                SortMode::SizeDesc => {
                    let size_a = get_file_size(&a.source);
                    let size_b = get_file_size(&b.source);
                    size_b.cmp(&size_a)
                }
                SortMode::Modified => {
                    let mod_a = get_file_modified(&a.source);
                    let mod_b = get_file_modified(&b.source);
                    mod_a.cmp(&mod_b)
                }
                SortMode::ModifiedDesc => {
                    let mod_a = get_file_modified(&a.source);
                    let mod_b = get_file_modified(&b.source);
                    mod_b.cmp(&mod_a)
                }
            };
            if cmp != std::cmp::Ordering::Equal {
                return cmp;
            }
        }

        // Final fallback: natural name sorting
        let name_a = a.source.file_name().unwrap_or_default();
        let name_b = b.source.file_name().unwrap_or_default();
        natord::compare_ignore_case(&name_a, &name_b)
    });
}

fn get_file_size(url: &MediaUrl) -> u64 {
    if let Some(path) = url.to_file_path()
        && let Ok(meta) = fs::metadata(path)
    {
        return meta.len();
    }
    0
}

fn get_file_modified(url: &MediaUrl) -> SystemTime {
    if let Some(path) = url.to_file_path()
        && let Ok(meta) = fs::metadata(path)
        && let Ok(time) = meta.modified()
    {
        return time;
    }
    SystemTime::UNIX_EPOCH
}

pub fn collect_image_paths(paths: Vec<PathBuf>) -> (Vec<PathBuf>, usize) {
    let mut resolved_paths = Vec::new();
    let mut start_idx = 0;

    if paths.len() == 1 && paths[0].is_file() {
        let target_file = &paths[0];

        if let Some(parent) = target_file.parent()
            && let Ok(entries) = fs::read_dir(parent)
        {
            let mut siblings = Vec::new();
            siblings.push(target_file.clone());
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type()
                    && file_type.is_file()
                {
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
            siblings.sort_by(|a, b| {
                natord::compare_ignore_case(
                    &a.file_name().unwrap_or_default().to_string_lossy(),
                    &b.file_name().unwrap_or_default().to_string_lossy(),
                )
            });
            if let Some(idx) = siblings.iter().position(|p| p == target_file) {
                start_idx = idx;
            }
            resolved_paths = siblings;
        } else {
            resolved_paths.push(target_file.clone());
        }
    } else {
        for path in paths {
            if path.is_dir() {
                if let Ok(entries) = fs::read_dir(path) {
                    let mut dir_paths = Vec::new();
                    for entry in entries.flatten() {
                        if let Ok(file_type) = entry.file_type()
                            && file_type.is_file()
                        {
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
                    dir_paths.sort_by(|a, b| {
                        natord::compare_ignore_case(
                            &a.file_name().unwrap_or_default().to_string_lossy(),
                            &b.file_name().unwrap_or_default().to_string_lossy(),
                        )
                    });
                    resolved_paths.extend(dir_paths);
                }
            } else {
                resolved_paths.push(path);
            }
        }
    }
    (resolved_paths, start_idx)
}

pub fn update_prefetch(animations: &mut [Animation], current_idx: usize, prefetch: usize) {
    let len = animations.len();
    if len == 0 {
        return;
    }

    let keep_forward = prefetch;
    let keep_backward = (prefetch / 2).max(1);

    // First, unparse everything outside the window
    for (i, anim) in animations.iter_mut().enumerate() {
        let is_in_window = if keep_forward + 1 + keep_backward >= len {
            true
        } else {
            let forward_dist = if i >= current_idx {
                i - current_idx
            } else {
                len - current_idx + i
            };

            let backward_dist = if current_idx >= i {
                current_idx - i
            } else {
                len - i + current_idx
            };

            forward_dist <= keep_forward || backward_dist <= keep_backward
        };

        if !is_in_window {
            anim.unparse();
        } else {
            anim.poll_parse();
        }
    }

    // Then, parse the first unparsed image in the window
    // We prioritize images closer to current_idx
    let mut parsed_one = false;
    for offset in 0..=keep_forward {
        if offset >= len {
            break;
        }
        let idx = (current_idx + offset) % len;
        if matches!(animations[idx].state, AnimationState::Skimmed) {
            let _ = animations[idx].parse();
            parsed_one = true;
            // Only parse one per call to avoid blocking the main thread too long
            break;
        }
    }

    if !parsed_one {
        for offset in 1..=keep_backward {
            if offset >= len {
                break;
            }
            let idx = (current_idx + len - offset) % len;
            if matches!(animations[idx].state, AnimationState::Skimmed) {
                let _ = animations[idx].parse();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_collect_image_paths_single_file() {
        let dir = tempdir().unwrap();
        let p1 = dir.path().join("a.jpg");
        let p2 = dir.path().join("b.png");
        let p3 = dir.path().join("c.txt");
        File::create(&p1).unwrap();
        File::create(&p2).unwrap();
        File::create(&p3).unwrap();

        let (paths, idx) = collect_image_paths(vec![p2.clone()]);
        assert_eq!(paths.len(), 2);
        // The list is sorted by file name lexicographically
        assert_eq!(paths[0], p1);
        assert_eq!(paths[1], p2);
        assert_eq!(idx, 1);
    }

    #[test]
    fn test_collect_image_paths_directory() {
        let dir = tempdir().unwrap();
        let p1 = dir.path().join("a.jpg");
        let p2 = dir.path().join("b.png");
        File::create(&p1).unwrap();
        File::create(&p2).unwrap();

        let (paths, idx) = collect_image_paths(vec![dir.path().to_path_buf()]);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&p1));
        assert!(paths.contains(&p2));
        assert_eq!(idx, 0); // When loading a directory, start index should be 0
    }
}
