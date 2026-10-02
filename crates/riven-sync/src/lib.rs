mod store;

use std::path::PathBuf;

pub use store::{Error, Store, Stored};

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
