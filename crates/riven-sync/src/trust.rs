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

/// The key pinned for `url`, or for another channel of the same pack: switching channels must not re-trust.
pub fn pinned(trusted: &Trusted, url: &str) -> Option<PublicKey> {
    trusted
        .keys
        .get(url)
        .or_else(|| {
            trusted
                .keys
                .iter()
                .find(|(pinned, _)| crate::remote::same_pack(pinned, url))
                .map(|(_, key)| key)
        })
        .and_then(|k| PublicKey::parse(k).ok())
}

/// Unpins the key of the pack at `url`, so its next install pins the one it serves; `false` if none was pinned.
pub fn forget(url: &str) -> Result<bool, TrustError> {
    let path = default_path()?;
    let mut trusted = load(&path)?;
    if !unpin(&mut trusted, url) {
        return Ok(false);
    }
    save(&path, &trusted)?;
    Ok(true)
}

fn unpin(trusted: &mut Trusted, url: &str) -> bool {
    let before = trusted.keys.len();
    trusted
        .keys
        .retain(|pinned, _| pinned != url && !crate::remote::same_pack(pinned, url));
    trusted.keys.len() != before
}

#[cfg(test)]
mod tests {
    use riven_format::KeyPair;

    use super::*;

    #[test]
    fn a_key_pinned_on_one_channel_holds_for_the_others() {
        let key = KeyPair::generate().unwrap().public();
        let mut trusted = Trusted::default();
        let stable = "https://x.test/pack/channels/stable.json";
        trusted.keys.insert(stable.into(), key.to_string());
        let beta = "https://x.test/pack/channels/beta.json";
        assert_eq!(pinned(&trusted, beta), Some(key));
        assert_eq!(
            pinned(&trusted, "https://x.test/other/channels/beta.json"),
            None
        );
        assert!(unpin(&mut trusted, beta));
        assert!(trusted.keys.is_empty());
    }
}
