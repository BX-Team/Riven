use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use riven_format::{EntryFile, Project};
use riven_resolve::JarFetcher;
use riven_sources::{Cache, GameMeta, Modrinth};
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

/// Fetches jars through the shared content store.
pub struct StoreJars {
    store: Store,
    http: reqwest::Client,
}

impl StoreJars {
    pub fn new() -> anyhow::Result<Self> {
        let store = Store::default_location().context("cannot locate the data directory")?;
        Ok(Self {
            store,
            http: riven_sources::client(),
        })
    }
}

impl JarFetcher for StoreJars {
    async fn jar(&self, file: &EntryFile) -> Result<Vec<u8>, String> {
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
