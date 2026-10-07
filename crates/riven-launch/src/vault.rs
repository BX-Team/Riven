use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead as _, Generate as _};
use chacha20poly1305::{KeyInit as _, XChaCha20Poly1305, XNonce};

use crate::{LaunchError, io};

const SERVICE: &str = "Riven Launcher";
const KEY_ENTRY: &str = "token-key";
const NONCE: usize = 24;

/// Secrets by account id, encrypted with a key kept in the OS keychain (Credential Manager caps secrets at 2.5 KB).
pub(crate) struct Vault {
    cipher: XChaCha20Poly1305,
    path: PathBuf,
}

impl Vault {
    pub(crate) fn open() -> Result<Self, LaunchError> {
        let dir = riven_sync::config_dir().ok_or(LaunchError::NoDir("config"))?;
        let key = key(&dir)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| LaunchError::Vault("bad key length".into()))?;
        Ok(Self {
            cipher,
            path: dir.join("tokens.bin"),
        })
    }

    fn read(&self) -> BTreeMap<String, String> {
        let Ok(bytes) = std::fs::read(&self.path) else {
            return BTreeMap::new();
        };
        let Some((nonce, sealed)) = bytes.split_at_checked(NONCE) else {
            return BTreeMap::new();
        };
        let Ok(nonce) = XNonce::try_from(nonce) else {
            return BTreeMap::new();
        };
        match self.cipher.decrypt(&nonce, sealed) {
            Ok(plain) => serde_json::from_slice(&plain).unwrap_or_default(),
            Err(_) => {
                tracing::warn!("saved sign-ins cannot be decrypted; accounts must sign in again");
                BTreeMap::new()
            }
        }
    }

    fn write(&self, secrets: &BTreeMap<String, String>) -> Result<(), LaunchError> {
        let nonce = XNonce::generate();
        let plain = serde_json::to_vec(secrets).unwrap_or_default();
        let sealed = self
            .cipher
            .encrypt(&nonce, plain.as_slice())
            .map_err(|_| LaunchError::Vault("cannot encrypt".into()))?;
        let mut bytes = nonce.to_vec();
        bytes.extend_from_slice(&sealed);
        write_private(&self.path, &bytes)
    }

    pub(crate) fn get(&self, account: &str) -> Option<String> {
        self.read().remove(account)
    }

    pub(crate) fn set(&self, account: &str, secret: &str) -> Result<(), LaunchError> {
        let mut secrets = self.read();
        secrets.insert(account.to_owned(), secret.to_owned());
        self.write(&secrets)
    }

    pub(crate) fn remove(&self, account: &str) -> Result<(), LaunchError> {
        let mut secrets = self.read();
        if secrets.remove(account).is_some() {
            self.write(&secrets)?;
        }
        Ok(())
    }
}

/// The vault key: the fallback file if one exists, else the OS keychain, else a new fallback file.
fn key(dir: &Path) -> Result<Vec<u8>, LaunchError> {
    let file = dir.join("token.key");
    if let Ok(text) = std::fs::read_to_string(&file) {
        return hex::decode(text.trim()).map_err(|_| LaunchError::Vault("bad token.key".into()));
    }
    let fresh = || {
        let mut key = vec![0u8; 32];
        getrandom::fill(&mut key).map_err(|e| LaunchError::Vault(e.to_string()))?;
        Ok::<_, LaunchError>(key)
    };
    let keychain = keyring::Entry::new(SERVICE, KEY_ENTRY).and_then(|entry| {
        match entry.get_password() {
            Ok(text) => Ok(hex::decode(text.trim()).ok()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e),
        }
        .map(|found| (entry, found))
    });
    match keychain {
        Ok((_, Some(key))) if key.len() == 32 => Ok(key),
        Ok((entry, _)) => {
            let key = fresh()?;
            match entry.set_password(&hex::encode(&key)) {
                Ok(()) => Ok(key),
                Err(e) => fallback(&file, key, &e),
            }
        }
        Err(e) => fallback(&file, fresh()?, &e),
    }
}

fn fallback(file: &Path, key: Vec<u8>, cause: &keyring::Error) -> Result<Vec<u8>, LaunchError> {
    tracing::warn!(
        "no OS keychain ({cause}); keeping the sign-in key in {} instead",
        file.display()
    );
    write_private(file, hex::encode(&key).as_bytes())?;
    Ok(key)
}

/// Writes a file only the current user can read, through a temporary file.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), LaunchError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    {
        use std::io::Write as _;
        let mut file = options.open(&tmp).map_err(io(&tmp))?;
        file.write_all(bytes).map_err(io(&tmp))?;
    }
    std::fs::rename(&tmp, path).map_err(io(path))
}
