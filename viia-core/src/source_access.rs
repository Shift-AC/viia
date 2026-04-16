use moka::sync::Cache;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

use crate::MediaUrl;
use crate::image_loader::EngineError;
use crate::sftp_transport;

pub enum SourceData {
    Bytes(Arc<Vec<u8>>),
    LocalFile(PathBuf),
}

impl SourceData {
    pub fn as_bytes(&self) -> Result<Arc<Vec<u8>>, EngineError> {
        match self {
            SourceData::Bytes(b) => Ok(b.clone()),
            SourceData::LocalFile(path) => {
                let bytes = std::fs::read(path)
                    .map_err(|e| EngineError::ImageLoadError(image::ImageError::IoError(e)))?;
                Ok(Arc::new(bytes))
            }
        }
    }
}

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
    pub fn read(&self, url: &MediaUrl) -> Result<SourceData, EngineError> {
        if url.scheme() == "file" {
            let path = url
                .to_file_path()
                .ok_or_else(|| EngineError::UnsupportedScheme("file".to_string()))?;
            return Ok(SourceData::LocalFile(path));
        }

        if let Some(hit) = self.bytes_cache.get(url.as_str()) {
            debug!("Source cache hit: {}", url.as_str());
            return Ok(SourceData::Bytes(hit));
        }

        info!("Source cache miss: {}", url.as_str());
        let bytes = match url.scheme().as_ref() {
            "sftp" => sftp_transport::download_file(url).map_err(EngineError::SourceError)?,
            other => return Err(EngineError::UnsupportedScheme(other.to_string())),
        };

        let arc = Arc::new(bytes);
        self.bytes_cache
            .insert(url.as_str().to_string(), arc.clone());
        Ok(SourceData::Bytes(arc))
    }

    pub fn read_all(&self, url: &MediaUrl) -> Result<Arc<Vec<u8>>, EngineError> {
        self.read(url)?.as_bytes()
    }
}
