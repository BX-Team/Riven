use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// On-disk cache of API responses keyed by URL, expiring after a TTL.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
    ttl: Duration,
}

impl Cache {
    pub fn new(dir: impl Into<PathBuf>, ttl: Duration) -> Self {
        Self {
            dir: dir.into(),
            ttl,
        }
    }

    fn path(&self, key: &str) -> PathBuf {
        let digest = Sha256::digest(key.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        self.dir.join(&hex[..2]).join(&hex)
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let path = self.path(key);
        let modified = fs::metadata(&path).ok()?.modified().ok()?;
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age > self.ttl {
            return None;
        }
        fs::read_to_string(path).ok()
    }

    pub fn put(&self, key: &str, body: &str) {
        let path = self.path(key);
        let write = || -> std::io::Result<()> {
            fs::create_dir_all(path.parent().expect("cache path has a parent"))?;
            let tmp = path.with_extension("tmp");
            fs::write(&tmp, body)?;
            fs::rename(tmp, &path)
        };
        if let Err(e) = write() {
            tracing::debug!("cache write {} failed: {e}", path.display());
        }
    }
}
