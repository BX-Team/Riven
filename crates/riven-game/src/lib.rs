pub mod java;

mod fabric;
mod forge;
mod install;
mod launch;
mod maven;
mod mojang;
mod net;
mod os;
mod processors;
mod version;

use std::path::{Path, PathBuf};

pub use install::install;
pub use launch::{Game, Launch, Session, Window, kill, launch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loader {
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

/// A Minecraft version and the loader build on top of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameVersion {
    pub minecraft: String,
    pub loader: Loader,
    /// Empty for vanilla.
    pub loader_version: String,
}

/// Where downloads shared by every instance live: libraries, assets, version files and Java.
#[derive(Debug, Clone)]
pub struct Dirs {
    root: PathBuf,
}

impl Dirs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn java(&self) -> PathBuf {
        self.root.join("java")
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Scratch space for files unpacked from loader installers.
    fn scratch(&self) -> PathBuf {
        self.root.join("tmp")
    }
}

/// What the game needs, read from Mojang's and the loader's metadata.
#[derive(Debug, Clone)]
pub struct Prepared {
    version: version::VersionJson,
    /// The vanilla version whose client jar runs.
    minecraft: String,
    installer: Option<forge::Installer>,
}

impl Prepared {
    /// The Java major version the game asks for.
    pub fn java_major(&self) -> u32 {
        self.version
            .java_version
            .as_ref()
            .map_or(8, |j| j.major_version)
    }

    /// Versions before 1.6 read assets from `resources` and pick their data folder themselves.
    fn before_1_6(&self) -> bool {
        self.version.assets.as_deref() == Some("pre-1.6")
    }

    /// Whether the game needs an x86-64 Java even on an ARM Mac: it ships no arm64 natives.
    pub fn needs_x64_java(&self) -> bool {
        cfg!(all(target_os = "macos", target_arch = "aarch64"))
            && !self.version.libraries.iter().any(|l| {
                l.name.contains("natives-macos-arm64")
                    || l.natives
                        .as_ref()
                        .is_some_and(|n| n.get("osx").is_some_and(|c| c.contains("arm64")))
            })
    }
}

/// Reads the metadata of `game`, fetching what is not cached yet; the cache keeps working offline.
pub async fn prepare(
    http: &reqwest::Client,
    dirs: &Dirs,
    game: &GameVersion,
) -> Result<Prepared, Error> {
    let vanilla = mojang::version(http, dirs, &game.minecraft).await?;
    let (child, installer) = match game.loader {
        Loader::Vanilla => (None, None),
        Loader::Fabric | Loader::Quilt => (Some(fabric::profile(http, dirs, game).await?), None),
        Loader::Forge | Loader::NeoForge => {
            let (child, installer) = forge::prepare(http, dirs, game).await?;
            (Some(child), Some(installer))
        }
    };
    let version = match child {
        Some(child) => child.inherit(vanilla),
        None => vanilla,
    };
    Ok(Prepared {
        version,
        minecraft: game.minecraft.clone(),
        installer,
    })
}

/// What happens while files are set up; amounts are in bytes, or steps for processors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Step(Step),
    Amount { done: u64, total: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Java,
    /// Libraries, assets and the client jar.
    Files,
    /// A Forge or NeoForge installer patching the game.
    Processors,
}

pub type Report<'a> = &'a (dyn Fn(Event) + Send + Sync);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{url}: {source}")]
    Http { url: String, source: reqwest::Error },
    #[error("{url} answered HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("{url} sent a file with the wrong checksum")]
    Checksum { url: String },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{what}: {source}")]
    Json {
        what: String,
        source: serde_json::Error,
    },
    #[error("{}: {source}", path.display())]
    Zip {
        path: PathBuf,
        source: zip::result::ZipError,
    },
    #[error("Minecraft {0} does not exist")]
    NoMinecraft(String),
    #[error("{0} does not exist")]
    NoLoader(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("Temurin has no Java {0} for this system")]
    NoJava(u32),
    #[error("{0} was not installed: {1}")]
    Missing(String, String),
    #[error("the loader installer failed: {0}")]
    Processor(String),
    #[error("a background task stopped: {0}")]
    Interrupted(String),
    #[error("cannot start {}: {source}", program.display())]
    Spawn {
        program: PathBuf,
        source: std::io::Error,
    },
}

pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

pub(crate) fn json<T: serde::de::DeserializeOwned>(what: &str, bytes: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|source| Error::Json {
        what: what.to_owned(),
        source,
    })
}

/// Runs blocking file work (hashing, unzipping) off the async threads.
pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|e| Err(Error::Interrupted(e.to_string())))
}
