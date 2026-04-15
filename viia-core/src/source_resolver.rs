use std::fs;
use std::path::{Path, PathBuf};

use tracing::warn;

use crate::MediaUrl;
use crate::sftp_transport::{self, ListingOrder};

pub fn resolve_media_urls(inputs: Vec<MediaUrl>) -> Result<(Vec<MediaUrl>, usize), String> {
    if inputs.is_empty() {
        return Ok((Vec::new(), 0));
    }

    if inputs.len() == 1 {
        let only = &inputs[0];
        return match only.scheme().as_ref() {
            "file" => resolve_single_file_url(only),
            "sftp" => resolve_single_sftp_file(only),
            other => Err(format!("Unsupported source scheme: {}", other)),
        };
    }

    let mut resolved = Vec::new();
    for input in inputs {
        match input.scheme().as_ref() {
            "file" => {
                let path = input
                    .to_file_path()
                    .ok_or_else(|| format!("Invalid file URL: {}", input.as_str()))?;
                if path.is_dir() {
                    resolved.extend(local_directory_children(path)?);
                } else {
                    resolved.push(input);
                }
            }
            "sftp" => resolved.push(input),
            other => return Err(format!("Unsupported source scheme: {}", other)),
        }
    }

    Ok((resolved, 0))
}

fn resolve_single_file_url(input: &MediaUrl) -> Result<(Vec<MediaUrl>, usize), String> {
    let path = input
        .to_file_path()
        .ok_or_else(|| format!("Invalid file URL: {}", input.as_str()))?;

    if path.is_file() {
        let target = MediaUrl::from_abs_path(&path)?;
        if let Some(parent) = path.parent() {
            match local_image_siblings(parent, &path) {
                Ok((siblings, idx)) if !siblings.is_empty() => return Ok((siblings, idx)),
                Ok(_) => {}
                Err(err) => warn!(
                    "Failed local sibling discovery for {}: {}",
                    input.as_str(),
                    err
                ),
            }
        }
        return Ok((vec![target], 0));
    }

    if path.is_dir() {
        return Ok((local_directory_children(path)?, 0));
    }

    Ok((vec![input.clone()], 0))
}

fn resolve_single_sftp_file(input: &MediaUrl) -> Result<(Vec<MediaUrl>, usize), String> {
    let parent = input
        .parent()
        .ok_or_else(|| format!("SFTP URL has no parent directory: {}", input.as_str()))?;
    let file_name = input
        .file_name()
        .ok_or_else(|| format!("SFTP URL has no file name: {}", input.as_str()))?;

    match sftp_transport::list_directory(&parent) {
        Ok(listing) => {
            let mut siblings = listing
                .entries
                .into_iter()
                .filter(|name| is_supported_image_name(name))
                .filter_map(|name| parent.join_file_name(&name))
                .collect::<Vec<_>>();

            if matches!(listing.order, ListingOrder::LexicographicFallback) {
                siblings.sort_by_key(|x| x.file_name().unwrap_or_default().to_ascii_lowercase());
            }

            if let Some(idx) = siblings
                .iter()
                .position(|candidate| candidate.file_name().as_deref() == Some(file_name.as_str()))
            {
                return Ok((siblings, idx));
            }

            warn!(
                "SFTP sibling discovery did not find target {}, falling back to explicit input",
                input.as_str()
            );
            Ok((vec![input.clone()], 0))
        }
        Err(err) => {
            warn!(
                "SFTP sibling discovery failed for {}: {}. Falling back to explicit input",
                input.as_str(),
                err
            );
            Ok((vec![input.clone()], 0))
        }
    }
}

fn local_image_siblings(parent: &Path, target: &Path) -> Result<(Vec<MediaUrl>, usize), String> {
    let target_url = MediaUrl::from_abs_path(target)?;
    let mut siblings = vec![target_url.clone()];

    let entries = fs::read_dir(parent)
        .map_err(|e| format!("Failed to read local directory {}: {}", parent.display(), e))?;
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        if path == target {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|x| x.to_str()) else {
            continue;
        };
        if !is_supported_image_name(file_name) {
            continue;
        }
        siblings.push(MediaUrl::from_abs_path(&path)?);
    }

    let start_idx = siblings
        .iter()
        .position(|candidate| candidate == &target_url)
        .unwrap_or(0);
    Ok((siblings, start_idx))
}

fn local_directory_children(path: PathBuf) -> Result<Vec<MediaUrl>, String> {
    let mut urls = Vec::new();
    let entries = fs::read_dir(&path)
        .map_err(|e| format!("Failed to read local directory {}: {}", path.display(), e))?;
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if !file_type.is_file() {
            continue;
        }
        let entry_path = entry.path();
        let Some(file_name) = entry_path.file_name().and_then(|x| x.to_str()) else {
            continue;
        };
        if is_supported_image_name(file_name) {
            urls.push(MediaUrl::from_abs_path(&entry_path)?);
        }
    }
    Ok(urls)
}

pub fn is_supported_image_name(name: &str) -> bool {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_is_supported_image_name() {
        assert!(is_supported_image_name("a.PNG"));
        assert!(is_supported_image_name("b.gif"));
        assert!(!is_supported_image_name("c.txt"));
    }

    #[test]
    fn test_resolve_single_file_url_discovers_siblings() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.png");
        let c = dir.path().join("c.txt");
        File::create(&a).unwrap();
        File::create(&b).unwrap();
        File::create(&c).unwrap();

        let target = MediaUrl::from_abs_path(&b).unwrap();
        let (resolved, idx) = resolve_media_urls(vec![target.clone()]).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[idx], target);
    }
}
