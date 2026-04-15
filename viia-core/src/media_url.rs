use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use url::Url;

/// Canonical URL identity for any media source.
///
/// - Local files are represented as `file://...` URLs.
/// - Remote files are represented as `sftp://...` URLs.
///
/// We store the normalized URL string for cheap `Clone`/`Hash` and parse to `Url` on demand.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MediaUrl(Arc<str>);

impl std::fmt::Display for MediaUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl MediaUrl {
    pub fn from_url(url: Url) -> Self {
        Self(Arc::from(url.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_url(&self) -> Result<Url, url::ParseError> {
        Url::parse(self.as_str())
    }

    pub fn scheme(&self) -> Cow<'_, str> {
        // Avoid parsing when possible.
        match self.as_str().split_once(':') {
            Some((scheme, _)) => Cow::Borrowed(scheme),
            None => Cow::Borrowed(""),
        }
    }

    pub fn parse_url(s: &str) -> Result<Self, url::ParseError> {
        // Ensure it parses, and re-serialize to get a normalized string.
        let url = Url::parse(s)?;
        Ok(Self(Arc::from(url.to_string())))
    }

    pub fn from_input(input: &str, cwd: &Path) -> Result<Self, String> {
        if input.contains("://") {
            let url = Url::parse(input).map_err(|e| format!("Invalid URL '{}': {}", input, e))?;
            return Ok(Self::from_url(url));
        }

        let path = PathBuf::from(input);
        let abs_path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        Self::from_abs_path(&abs_path)
    }

    /// Converts an absolute local filesystem path into a `file://...` URL.
    ///
    /// This does not check for existence.
    pub fn from_abs_path(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err(format!(
                "Path must be absolute for file:// URL: {}",
                path.display()
            ));
        }
        let url = Url::from_file_path(path).map_err(|_| {
            format!(
                "Failed to convert local path to file:// URL: {}",
                path.display()
            )
        })?;
        Ok(Self(Arc::from(url.to_string())))
    }

    /// Converts a `file://...` URL into a local filesystem path.
    pub fn to_file_path(&self) -> Option<PathBuf> {
        let url = self.to_url().ok()?;
        if url.scheme() != "file" {
            return None;
        }
        url.to_file_path().ok()
    }

    /// Returns the last path segment (filename) if present.
    pub fn file_name(&self) -> Option<String> {
        let url = self.to_url().ok()?;
        url.path_segments()
            .and_then(|mut segs| segs.next_back())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    pub fn parent(&self) -> Option<Self> {
        let mut url = self.to_url().ok()?;
        let mut segments = url
            .path_segments()
            .map(|s| s.map(|x| x.to_string()).collect::<Vec<_>>())?;

        if segments.is_empty() {
            return None;
        }

        if self.as_str().ends_with('/') {
            segments.pop();
        } else {
            let _ = segments.pop();
        }

        url.path_segments_mut().ok()?.clear();
        for segment in &segments {
            url.path_segments_mut().ok()?.push(segment);
        }
        if !url.path().ends_with('/') {
            let mut path = url.path().to_string();
            path.push('/');
            url.set_path(&path);
        }
        Some(Self::from_url(url))
    }

    pub fn join_file_name(&self, file_name: &str) -> Option<Self> {
        let mut url = self.to_url().ok()?;
        {
            let mut segments = url.path_segments_mut().ok()?;
            segments.pop_if_empty();
            segments.push(file_name);
        }
        Some(Self::from_url(url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_from_input_local_path_becomes_file_url() {
        let dir = tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let url = MediaUrl::from_input("image.png", &cwd).unwrap();
        assert_eq!(url.scheme(), "file");
        assert!(url.as_str().contains("image.png"));
    }

    #[test]
    fn test_from_input_sftp_url_is_preserved() {
        let cwd = PathBuf::from("/tmp");
        let url = MediaUrl::from_input("sftp://example.com/path/image.png", &cwd).unwrap();
        assert_eq!(url.scheme(), "sftp");
        assert_eq!(url.file_name().as_deref(), Some("image.png"));
    }

    #[test]
    fn test_parent_and_join_round_trip() {
        let url = MediaUrl::parse_url("sftp://example.com/path/image.png").unwrap();
        let parent = url.parent().unwrap();
        assert_eq!(parent.as_str(), "sftp://example.com/path/");
        let rebuilt = parent.join_file_name("image.png").unwrap();
        assert_eq!(rebuilt, url);
    }
}
