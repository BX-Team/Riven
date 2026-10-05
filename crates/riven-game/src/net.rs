use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{StreamExt as _, TryStreamExt as _, stream};
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use tokio::io::AsyncWriteExt as _;

use crate::{Error, Event, Report, io};

const ATTEMPTS: u32 = 3;
const PARALLEL: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checksum {
    Sha1(String),
    Sha256(String),
}

/// A file to download, from the first of `urls` that has it.
#[derive(Debug, Clone)]
pub struct Download {
    pub urls: Vec<String>,
    pub path: PathBuf,
    pub checksum: Option<Checksum>,
    pub size: Option<u64>,
}

impl Download {
    pub fn new(url: impl Into<String>, path: PathBuf) -> Self {
        Self {
            urls: vec![url.into()],
            path,
            checksum: None,
            size: None,
        }
    }

    pub fn sha1(mut self, sha1: Option<&str>) -> Self {
        self.checksum = sha1.map(|s| Checksum::Sha1(s.to_owned()));
        self
    }

    pub fn size(mut self, size: Option<u64>) -> Self {
        self.size = size.filter(|s| *s > 0);
        self
    }

    /// Whether the file on disk already is this one; sizes are trusted, as files are written whole.
    pub fn is_done(&self) -> bool {
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return false;
        };
        match (self.size, &self.checksum) {
            (Some(size), _) => meta.len() == size,
            (None, Some(Checksum::Sha1(sha1))) => {
                sha1_file(&self.path).is_ok_and(|h| h.eq_ignore_ascii_case(sha1))
            }
            _ => meta.len() > 0,
        }
    }
}

pub async fn get(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, Error> {
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            backoff(attempt).await;
        }
        match get_once(http, url).await {
            Ok(bytes) => return Ok(bytes),
            Err(
                e @ Error::Status {
                    status: 400..=499, ..
                },
            ) => return Err(e),
            Err(e) => last = Some(e),
        }
    }
    Err(last.expect("at least one attempt ran"))
}

async fn get_once(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, Error> {
    let failed = |source| Error::Http {
        url: url.to_owned(),
        source,
    };
    let response = http.get(url).send().await.map_err(failed)?;
    check(url, &response)?;
    Ok(response.bytes().await.map_err(failed)?.to_vec())
}

pub async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, Error> {
    crate::json(url, &get(http, url).await?)
}

fn check(url: &str, response: &reqwest::Response) -> Result<(), Error> {
    if response.status().is_success() {
        Ok(())
    } else {
        Err(Error::Status {
            url: url.to_owned(),
            status: response.status().as_u16(),
        })
    }
}

async fn backoff(attempt: u32) {
    tokio::time::sleep(Duration::from_millis(500 << attempt)).await;
}

/// Downloads every file, several at once, reporting bytes as they arrive.
pub async fn fetch_all(
    http: &reqwest::Client,
    files: &[Download],
    report: Report<'_>,
) -> Result<(), Error> {
    let total: u64 = files.iter().filter_map(|f| f.size).sum();
    let done = AtomicU64::new(0);
    report(Event::Amount { done: 0, total });
    let progress = |bytes: u64| {
        let done = done.fetch_add(bytes, Ordering::Relaxed) + bytes;
        report(Event::Amount {
            done: done.min(total),
            total,
        });
    };
    stream::iter(files.iter().map(Ok))
        .try_for_each_concurrent(PARALLEL, |file| fetch(http, file, &progress))
        .await
}

/// Downloads one file through a temporary name, so a half-written file never looks finished.
pub async fn fetch(
    http: &reqwest::Client,
    file: &Download,
    progress: &(dyn Fn(u64) + Sync),
) -> Result<(), Error> {
    let mut last = None;
    for url in &file.urls {
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                backoff(attempt).await;
            }
            match fetch_once(http, url, file, progress).await {
                Ok(()) => return Ok(()),
                Err(
                    e @ Error::Status {
                        status: 400..=499, ..
                    },
                ) => {
                    last = Some(e);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
    }
    Err(last.unwrap_or_else(|| {
        Error::Missing(
            file.path.display().to_string(),
            "it has no download link".into(),
        )
    }))
}

enum Hasher {
    Sha1(Sha1),
    Sha256(Sha256),
}

impl Hasher {
    fn update(&mut self, bytes: &[u8]) {
        match self {
            Hasher::Sha1(h) => h.update(bytes),
            Hasher::Sha256(h) => h.update(bytes),
        }
    }
}

async fn fetch_once(
    http: &reqwest::Client,
    url: &str,
    file: &Download,
    progress: &(dyn Fn(u64) + Sync),
) -> Result<(), Error> {
    let failed = |source| Error::Http {
        url: url.to_owned(),
        source,
    };
    let response = http.get(url).send().await.map_err(failed)?;
    check(url, &response)?;
    if let Some(parent) = file.path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(io(parent))?;
    }
    let part = part_path(&file.path);
    let mut out = tokio::fs::File::create(&part).await.map_err(io(&part))?;
    let mut hasher = match &file.checksum {
        Some(Checksum::Sha256(_)) => Hasher::Sha256(Sha256::new()),
        _ => Hasher::Sha1(Sha1::new()),
    };
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(failed)?;
        hasher.update(&chunk);
        out.write_all(&chunk).await.map_err(io(&part))?;
        progress(chunk.len() as u64);
    }
    out.flush().await.map_err(io(&part))?;
    drop(out);
    let digest = match hasher {
        Hasher::Sha1(h) => hex::encode(h.finalize()),
        Hasher::Sha256(h) => hex::encode(h.finalize()),
    };
    let expected = match &file.checksum {
        Some(Checksum::Sha1(s) | Checksum::Sha256(s)) => Some(s),
        None => None,
    };
    if expected.is_some_and(|s| !s.eq_ignore_ascii_case(&digest)) {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(Error::Checksum {
            url: url.to_owned(),
        });
    }
    tokio::fs::rename(&part, &file.path)
        .await
        .map_err(io(&file.path))
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".part");
    path.with_file_name(name)
}

/// Writes a whole file through a temporary name.
pub fn save(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let part = part_path(path);
    std::fs::write(&part, bytes).map_err(io(&part))?;
    std::fs::rename(&part, path).map_err(io(path))
}

pub fn sha1_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buf[..n]);
    }
}

pub fn sha1_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}
