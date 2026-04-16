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

        let disk_cache_dir = std::env::temp_dir().join("viia_file_cache");
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&url.as_str(), &mut hasher);
        let url_hash = format!("{:016x}", std::hash::Hasher::finish(&hasher));

        let data_path = disk_cache_dir.join(format!("{}.data", url_hash));
        let meta_path = disk_cache_dir.join(format!("{}.meta", url_hash));

        let remote_metadata = match url.scheme().as_ref() {
            "sftp" => sftp_transport::get_metadata(url).map_err(EngineError::SourceError)?,
            other => return Err(EngineError::UnsupportedScheme(other.to_string())),
        };

        if let Ok(local_meta) = std::fs::read_to_string(&meta_path)
            && local_meta == remote_metadata
            && data_path.exists()
        {
            debug!("Source disk cache hit: {}", url.as_str());
            if let Ok(bytes) = std::fs::read(&data_path) {
                let arc = Arc::new(bytes);
                self.bytes_cache
                    .insert(url.as_str().to_string(), arc.clone());
                return Ok(SourceData::Bytes(arc));
            }
        }

        info!("Source cache miss: {}", url.as_str());
        let bytes = match url.scheme().as_ref() {
            "sftp" => sftp_transport::download_file(url).map_err(EngineError::SourceError)?,
            other => return Err(EngineError::UnsupportedScheme(other.to_string())),
        };

        if let Err(e) = std::fs::create_dir_all(&disk_cache_dir) {
            tracing::warn!("Failed to create disk cache dir: {}", e);
        } else {
            if let Err(e) = std::fs::write(&data_path, &bytes) {
                tracing::warn!("Failed to write disk cache data: {}", e);
            }
            if let Err(e) = std::fs::write(&meta_path, &remote_metadata) {
                tracing::warn!("Failed to write disk cache meta: {}", e);
            }
        }

        let arc = Arc::new(bytes);
        self.bytes_cache
            .insert(url.as_str().to_string(), arc.clone());
        Ok(SourceData::Bytes(arc))
    }

    pub fn read_all(&self, url: &MediaUrl) -> Result<Arc<Vec<u8>>, EngineError> {
        self.read(url)?.as_bytes()
    }
}

pub fn clear_disk_cache() {
    let disk_cache_dir = std::env::temp_dir().join("viia_file_cache");
    if disk_cache_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&disk_cache_dir) {
            tracing::warn!("Failed to clear disk cache directory: {}", e);
        } else {
            tracing::info!("Disk cache cleared successfully.");
        }
    }
}
