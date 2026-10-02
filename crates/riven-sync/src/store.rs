use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use futures_util::StreamExt;
use riven_format::{Hash, Hashes};
use sha1::Sha1;
use sha2::{Digest, Sha512};
use tokio::io::AsyncWriteExt;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("download of {url} failed: {source}")]
    Http { url: String, source: reqwest::Error },
    #[error("{url} returned {status}")]
    Status {
        url: String,
        status: reqwest::StatusCode,
    },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{origin} has {algo} {actual}, expected {expected}")]
    HashMismatch {
        origin: String,
        algo: &'static str,
        expected: String,
        actual: String,
    },
    #[error("no download url")]
    NoUrls,
    #[error("cannot verify {0}: no sha512 or sha1 given")]
    NoHash(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub path: PathBuf,
    pub sha512: String,
    pub sha1: String,
    pub size: u64,
}

/// Content-addressed file store: `<root>/sha512/ab/abcdef…`.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `<data_dir>/store`.
    pub fn default_location() -> Option<Self> {
        crate::data_dir().map(|d| Self::new(d.join("store")))
    }

    pub fn path(&self, sha512: &str) -> PathBuf {
        let prefix = sha512.get(..2).unwrap_or("00");
        self.root.join("sha512").join(prefix).join(sha512)
    }

    /// The stored file for `hashes`, if already present.
    pub fn get(&self, hashes: &Hashes) -> Option<PathBuf> {
        let path = self.path(hashes.sha512.as_deref()?);
        path.is_file().then_some(path)
    }

    fn temp_path(&self) -> PathBuf {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.root
            .join("tmp")
            .join(format!("{}-{n}.part", std::process::id()))
    }

    /// Downloads into the store unless present, trying `urls` in order and verifying `expected`.
    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        urls: &[String],
        expected: &Hashes,
    ) -> Result<Stored, Error> {
        if let Some(path) = self.get(expected) {
            return self.stat(path, expected);
        }
        let mut last = Error::NoUrls;
        for url in urls {
            match self.download(http, url, expected).await {
                Ok(stored) => return Ok(stored),
                Err(e) => {
                    tracing::warn!("{e}");
                    last = e;
                }
            }
        }
        Err(last)
    }

    fn stat(&self, path: PathBuf, hashes: &Hashes) -> Result<Stored, Error> {
        let size = std::fs::metadata(&path).map_err(io(&path))?.len();
        Ok(Stored {
            sha512: hashes.sha512.clone().unwrap_or_default(),
            sha1: hashes.sha1.clone().unwrap_or_default(),
            path,
            size,
        })
    }

    async fn download(
        &self,
        http: &reqwest::Client,
        url: &str,
        expected: &Hashes,
    ) -> Result<Stored, Error> {
        let http_err = |source| Error::Http {
            url: url.to_owned(),
            source,
        };
        let response = http.get(url).send().await.map_err(http_err)?;
        if !response.status().is_success() {
            return Err(Error::Status {
                url: url.to_owned(),
                status: response.status(),
            });
        }

        let temp = self.temp_path();
        let parent = temp.parent().expect("temp path has a parent");
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(io(parent))?;
        let mut file = tokio::fs::File::create(&temp).await.map_err(io(&temp))?;
        let mut hasher = Hasher::default();
        let mut stream = response.bytes_stream();
        let result = async {
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(http_err)?;
                hasher.update(&chunk);
                file.write_all(&chunk).await.map_err(io(&temp))?;
            }
            file.flush().await.map_err(io(&temp))
        }
        .await;
        drop(file);
        let stored = match result {
            Ok(()) => self.commit(&temp, hasher, expected, url),
            Err(e) => Err(e),
        };
        if stored.is_err() {
            let _ = tokio::fs::remove_file(&temp).await;
        }
        stored
    }

    /// Adds in-memory bytes (local files, tests), verifying `expected` when it has a hash.
    pub fn insert(&self, bytes: &[u8], expected: &Hashes, origin: &str) -> Result<Stored, Error> {
        let mut hasher = Hasher::default();
        hasher.update(bytes);
        let temp = self.temp_path();
        let parent = temp.parent().expect("temp path has a parent");
        std::fs::create_dir_all(parent).map_err(io(parent))?;
        std::fs::write(&temp, bytes).map_err(io(&temp))?;
        let stored = self.commit(&temp, hasher, expected, origin);
        if stored.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        stored
    }

    fn commit(
        &self,
        temp: &Path,
        hasher: Hasher,
        expected: &Hashes,
        origin: &str,
    ) -> Result<Stored, Error> {
        let actual = hasher.finish();
        let mismatch = |algo, expected: &str, actual: &str| Error::HashMismatch {
            origin: origin.to_owned(),
            algo,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        };
        match expected.strongest() {
            Some(Hash::Sha512(h)) if !h.eq_ignore_ascii_case(&actual.sha512) => {
                return Err(mismatch("sha512", h, &actual.sha512));
            }
            Some(Hash::Sha1(h)) if !h.eq_ignore_ascii_case(&actual.sha1) => {
                return Err(mismatch("sha1", h, &actual.sha1));
            }
            _ => {}
        }
        let path = self.path(&actual.sha512);
        let parent = path.parent().expect("store path has a parent");
        std::fs::create_dir_all(parent).map_err(io(parent))?;
        std::fs::rename(temp, &path).map_err(io(&path))?;
        Ok(Stored {
            path,
            sha512: actual.sha512,
            sha1: actual.sha1,
            size: actual.size,
        })
    }
}

#[derive(Default)]
struct Hasher {
    sha512: Sha512,
    sha1: Sha1,
    size: u64,
}

struct Digests {
    sha512: String,
    sha1: String,
    size: u64,
}

impl Hasher {
    fn update(&mut self, bytes: &[u8]) {
        self.sha512.update(bytes);
        self.sha1.update(bytes);
        self.size += bytes.len() as u64;
    }

    fn finish(self) -> Digests {
        Digests {
            sha512: hex::encode(self.sha512.finalize()),
            sha1: hex::encode(self.sha1.finalize()),
            size: self.size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_verifies_and_addresses_by_sha512() {
        let dir = std::env::temp_dir().join(format!("riven-store-{}", std::process::id()));
        let store = Store::new(&dir);
        let bytes = b"hello riven";
        let stored = store.insert(bytes, &Hashes::default(), "test").unwrap();
        assert_eq!(stored.path, store.path(&stored.sha512));
        assert_eq!(std::fs::read(&stored.path).unwrap(), bytes);
        assert_eq!(stored.sha1, "9256dc4430cb8ef773654745bc822bd9efa4172a");

        let wrong = Hashes {
            sha1: Some("0".repeat(40)),
            ..Hashes::default()
        };
        assert!(matches!(
            store.insert(b"other", &wrong, "test"),
            Err(Error::HashMismatch { algo: "sha1", .. })
        ));
        let leftovers = std::fs::read_dir(dir.join("tmp")).unwrap().count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
