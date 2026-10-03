use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use riven_format::{Entry, Hashes, Project, Source};
use riven_resolve::{Downloaded, Downloader, JarFetcher};
use riven_sources::{Cache, GameMeta, GitHub, Modrinth};
use riven_sync::Store;

pub const PROJECT_FILE: &str = "riven.json";
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// A loaded `riven.json` and the directory it lives in.
pub struct Workspace {
    pub dir: PathBuf,
    pub project: Project,
}

impl Workspace {
    /// Finds `riven.json` in the current directory or its parents.
    pub fn find() -> anyhow::Result<Self> {
        let cwd = std::env::current_dir().context("cannot read the current directory")?;
        let Some(dir) = cwd.ancestors().find(|d| d.join(PROJECT_FILE).is_file()) else {
            bail!("no {PROJECT_FILE} here or in any parent directory; run `riven init`");
        };
        let path = dir.join(PROJECT_FILE);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let project = riven_format::from_str(&text).with_context(|| path.display().to_string())?;
        Ok(Self {
            dir: dir.to_owned(),
            project,
        })
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let issues = self.project.validate();
        if let Some(issue) = issues.first() {
            bail!("refusing to write an invalid {PROJECT_FILE}: {issue}");
        }
        write_atomic(
            &self.dir.join(PROJECT_FILE),
            &riven_format::to_string(&self.project),
        )
    }

    pub fn cache(&self) -> Cache {
        Cache::new(self.dir.join(".riven").join("cache"), CACHE_TTL)
    }

    pub fn modrinth(&self) -> Modrinth {
        Modrinth::new(riven_sources::client()).with_cache(self.cache())
    }

    pub fn github(&self) -> GitHub {
        GitHub::new(riven_sources::client()).with_cache(self.cache())
    }

    /// Jar access for this pack: the content store, plus `local` files from the repository.
    pub fn jars(&self) -> anyhow::Result<StoreJars> {
        StoreJars::new(&self.dir)
    }
}

pub fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))
}

pub fn game_meta(cache: Option<Cache>) -> GameMeta {
    let game = GameMeta::new(riven_sources::client());
    match cache {
        Some(cache) => game.with_cache(cache),
        None => game,
    }
}

/// Fetches jars through the shared content store; `local` entries come from the pack repository.
pub struct StoreJars {
    store: Store,
    http: reqwest::Client,
    repo: PathBuf,
}

impl StoreJars {
    pub fn new(repo: &Path) -> anyhow::Result<Self> {
        let store = Store::default_location().context("cannot locate the data directory")?;
        Ok(Self {
            store,
            http: riven_sources::client(),
            repo: repo.to_owned(),
        })
    }

    /// Adds bytes to the store, returning them with their computed hashes.
    pub fn keep(&self, bytes: Vec<u8>, origin: &str) -> Result<Downloaded, String> {
        let stored = self
            .store
            .insert(&bytes, &Hashes::default(), origin)
            .map_err(|e| e.to_string())?;
        Ok(Downloaded {
            bytes,
            hashes: Hashes {
                sha512: Some(stored.sha512),
                sha1: Some(stored.sha1),
            },
            size: stored.size,
        })
    }
}

impl StoreJars {
    /// Where `entry`'s bytes are on disk, if already present.
    pub fn local_path(&self, entry: &Entry) -> Option<PathBuf> {
        match &entry.source {
            Source::Local { path } => Some(self.repo.join(path.as_str())),
            _ => self.store.get(&entry.file.hashes),
        }
    }
}

impl Downloader for StoreJars {
    async fn download(&self, url: &str) -> Result<Downloaded, String> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| e.to_string())?;
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        self.keep(bytes.to_vec(), url)
    }
}

impl JarFetcher for StoreJars {
    async fn jar(&self, entry: &Entry) -> Result<Vec<u8>, String> {
        if let Source::Local { path } = &entry.source {
            let path = self.repo.join(path.as_str());
            return tokio::fs::read(&path)
                .await
                .map_err(|e| format!("{}: {e}", path.display()));
        }
        let file = &entry.file;
        let path = match self.store.get(&file.hashes) {
            Some(path) => path,
            None => {
                let urls: Vec<String> = file.url.iter().cloned().collect();
                self.store
                    .fetch(&self.http, &urls, &file.hashes)
                    .await
                    .map_err(|e| e.to_string())?
                    .path
            }
        };
        tokio::fs::read(&path).await.map_err(|e| e.to_string())
    }
}
