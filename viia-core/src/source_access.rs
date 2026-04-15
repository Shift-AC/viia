use moka::sync::Cache;
use std::sync::Arc;
use tracing::{debug, info};

use crate::MediaUrl;
use crate::image_loader::EngineError;
use crate::sftp_transport;

#[derive(Clone)]
pub struct SourceAccess {
    // URL string -> file bytes
    bytes_cache: Cache<String, Arc<Vec<u8>>>,
}

impl Default for SourceAccess {
    fn default() -> Self {
        Self {
            bytes_cache: Cache::new(256),
        }
    }
}

impl SourceAccess {
    pub fn read_all(&self, url: &MediaUrl) -> Result<Arc<Vec<u8>>, EngineError> {
        if let Some(hit) = self.bytes_cache.get(url.as_str()) {
            debug!("Source cache hit: {}", url.as_str());
            return Ok(hit);
        }

        info!("Source cache miss: {}", url.as_str());
        let bytes = match url.scheme().as_ref() {
            "file" => {
                let path = url
                    .to_file_path()
                    .ok_or_else(|| EngineError::UnsupportedScheme("file".to_string()))?;
                std::fs::read(&path)
                    .map_err(|e| EngineError::ImageLoadError(image::ImageError::IoError(e)))?
            }
            "sftp" => sftp_transport::download_file(url).map_err(EngineError::SourceError)?,
            other => return Err(EngineError::UnsupportedScheme(other.to_string())),
        };

        let arc = Arc::new(bytes);
        self.bytes_cache
            .insert(url.as_str().to_string(), arc.clone());
        Ok(arc)
    }
}
