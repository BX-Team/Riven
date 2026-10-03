use std::path::{Path, PathBuf};

use riven_format::{PublicKey, Trusted};

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Format(#[from] riven_format::Error),
    #[error("cannot locate the config directory")]
    NoConfigDir,
}

/// `<config>/riven/trusted.json`.
pub fn default_path() -> Result<PathBuf, TrustError> {
    crate::config_dir()
        .map(|d| d.join("trusted.json"))
        .ok_or(TrustError::NoConfigDir)
}

/// Pinned keys per pack URL; a missing file means nothing is pinned yet.
pub fn load(path: &Path) -> Result<Trusted, TrustError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(riven_format::from_str(&text)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Trusted::default()),
        Err(source) => Err(TrustError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

pub fn save(path: &Path, trusted: &Trusted) -> Result<(), TrustError> {
    let io = |source| TrustError::Io {
        path: path.to_owned(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, riven_format::to_string(trusted)).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

pub fn pinned(trusted: &Trusted, url: &str) -> Option<PublicKey> {
    trusted.keys.get(url).and_then(|k| PublicKey::parse(k).ok())
}
