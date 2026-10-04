pub mod install;
pub mod remote;
mod store;
pub mod trust;
pub mod update;

use std::path::PathBuf;

use riven_format::PackPath;

pub use store::{Error, Store, Stored};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("`{0}` is not a pack link (an http(s) URL of the pack or a channel)")]
    BadLink(String),
    #[error("request to {url} failed: {source}")]
    Http { url: String, source: reqwest::Error },
    #[error("{url} returned {status}")]
    Status {
        url: String,
        status: reqwest::StatusCode,
    },
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Format(#[from] riven_format::Error),
    #[error(transparent)]
    Key(#[from] riven_format::SignError),
    #[error(
        "the pack is signed with a different key than before; if the author rotated it, run `riven trust reset <url>`"
    )]
    KeyChanged,
    #[error("the {0} is not signed, but this pack was; refusing a possible forgery")]
    Unsigned(String),
    #[error("the {0} signature does not match the pack's key")]
    BadSignature(String),
    #[error("{0} does not match the hash its channel pointer names")]
    ManifestMismatch(String),
    #[error("not a valid .riven archive: {0}")]
    Archive(String),
    #[error("no group `{0}` in this pack")]
    UnknownGroup(String),
    #[error("several release files install to `{0}`")]
    Conflict(PackPath),
    #[error("{} is a symlink; riven does not write through symlinks", .0.display())]
    Symlink(PathBuf),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Store(#[from] Error),
    #[error(transparent)]
    Trust(#[from] trust::TrustError),
    #[error("nothing is installed in {}; pass a pack link", .0.display())]
    NothingInstalled(PathBuf),
}

/// `~/.local/share/riven`, `%APPDATA%\Riven`, `~/Library/Application Support/Riven`.
pub fn data_dir() -> Option<PathBuf> {
    let name = if cfg!(target_os = "linux") {
        "riven"
    } else {
        "Riven"
    };
    dirs::data_dir().map(|d| d.join(name))
}

/// `~/.config/riven` and the platform equivalents.
pub fn config_dir() -> Option<PathBuf> {
    let name = if cfg!(target_os = "linux") {
        "riven"
    } else {
        "Riven"
    };
    dirs::config_dir().map(|d| d.join(name))
}
