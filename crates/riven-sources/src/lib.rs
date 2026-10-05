mod cache;
mod game;
pub mod github;
mod http;
mod identify;
pub mod modrinth;

use std::collections::HashMap;
use std::future::Future;

use riven_format::{Hashes, Kind, LoaderKind, Side, SourceKind};

pub use cache::Cache;
pub use game::GameMeta;
pub use github::GitHub;
pub use http::{Error, client};
pub use identify::{Known, Matched, identify};
pub use modrinth::Modrinth;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The loader's name as platforms and `riven.json` spell it.
pub fn loader_name(loader: LoaderKind) -> &'static str {
    match loader {
        LoaderKind::Fabric => "fabric",
        LoaderKind::Quilt => "quilt",
        LoaderKind::Forge => "forge",
        LoaderKind::NeoForge => "neoforge",
    }
}

/// What content is being looked for: game version, loader and content kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub minecraft: String,
    pub loader: LoaderKind,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Required,
    Optional,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub kind: Option<Kind>,
    pub client: Support,
    pub server: Support,
    /// Team or organization owning the project; a change is worth a warning on update.
    pub owner: String,
    pub icon_url: Option<String>,
}

impl ProjectInfo {
    /// The side the project should be installed on, from its declared support.
    pub fn side(&self) -> Side {
        match (self.client, self.server) {
            (Support::Unsupported, _) => Side::Server,
            (_, Support::Unsupported) => Side::Client,
            _ => Side::Both,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Channel {
    Release,
    Beta,
    Alpha,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub id: String,
    pub project: String,
    pub name: String,
    pub number: String,
    pub channel: Channel,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    /// RFC 3339 timestamp.
    pub published: String,
    pub files: Vec<VersionFile>,
    pub dependencies: Vec<Dependency>,
}

impl Version {
    pub fn primary_file(&self) -> Option<&VersionFile> {
        self.files
            .iter()
            .find(|f| f.primary)
            .or_else(|| self.files.first())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionFile {
    pub filename: String,
    /// `None` when the file may not be redistributed.
    pub url: Option<String>,
    pub size: u64,
    pub hashes: Hashes,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub project: Option<String>,
    pub version: Option<String>,
    pub kind: DependencyKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Required,
    Optional,
    Incompatible,
    Embedded,
}

/// A content platform Riven can add entries from.
pub trait Source: Send + Sync {
    fn kind(&self) -> SourceKind;

    fn search(
        &self,
        query: &str,
        target: &Target,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Hit>>> + Send;

    fn project(&self, id: &str) -> impl Future<Output = Result<ProjectInfo>> + Send;

    /// Versions of `project` compatible with `target`, newest first.
    fn resolve(
        &self,
        project: &str,
        target: &Target,
    ) -> impl Future<Output = Result<Vec<Version>>> + Send;

    fn version(&self, id: &str) -> impl Future<Output = Result<Version>> + Send;

    /// Several projects at once; unknown ids are left out.
    fn projects(&self, ids: &[String]) -> impl Future<Output = Result<Vec<ProjectInfo>>> + Send {
        async move {
            let mut out = Vec::new();
            for id in ids {
                match self.project(id).await {
                    Ok(project) => out.push(project),
                    Err(Error::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(out)
        }
    }

    /// Several versions at once; unknown ids are left out.
    fn versions(&self, ids: &[String]) -> impl Future<Output = Result<Vec<Version>>> + Send {
        async move {
            let mut out = Vec::new();
            for id in ids {
                match self.version(id).await {
                    Ok(version) => out.push(version),
                    Err(Error::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(out)
        }
    }

    /// The newest version compatible with `target` for each of `current`, keyed by its id.
    fn latest(
        &self,
        current: &[Version],
        target: &Target,
    ) -> impl Future<Output = Result<HashMap<String, Version>>> + Send {
        async move {
            let mut out = HashMap::new();
            for version in current {
                let newest = self
                    .resolve(&version.project, target)
                    .await?
                    .into_iter()
                    .next();
                if let Some(newest) = newest {
                    out.insert(version.id.clone(), newest);
                }
            }
            Ok(out)
        }
    }

    fn dependencies(
        &self,
        version: &Version,
    ) -> impl Future<Output = Result<Vec<Dependency>>> + Send {
        let deps = version.dependencies.clone();
        async move { Ok(deps) }
    }
}
