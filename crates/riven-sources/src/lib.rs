mod cache;
mod http;
pub mod modrinth;

use std::future::Future;

use riven_format::{Hashes, Kind, LoaderKind, Side, SourceKind};

pub use cache::Cache;
pub use http::{Error, client};
pub use modrinth::Modrinth;

pub type Result<T, E = Error> = std::result::Result<T, E>;

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

    fn dependencies(
        &self,
        version: &Version,
    ) -> impl Future<Output = Result<Vec<Dependency>>> + Send {
        let deps = version.dependencies.clone();
        async move { Ok(deps) }
    }
}
