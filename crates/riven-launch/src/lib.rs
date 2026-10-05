pub mod accounts;
pub mod instances;
pub mod mods;

use std::path::{Path, PathBuf};

use riven_format::{Document, Settings};

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("cannot locate the {0} directory")]
    NoDir(&'static str),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Format {
        path: PathBuf,
        source: riven_format::Error,
    },
    #[error("no instance `{0}`")]
    NoInstance(String),
    #[error("`{0}` is not a valid player name (3–16 letters, digits or _)")]
    BadName(String),
}

pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> LaunchError + '_ {
    move |source| LaunchError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Reads a document, or its default when the file does not exist yet.
pub(crate) fn load_or_default<T: Document + Default>(path: &Path) -> Result<T, LaunchError> {
    match std::fs::read_to_string(path) {
        Ok(text) => riven_format::from_str(&text).map_err(|source| LaunchError::Format {
            path: path.to_owned(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(io(path)(e)),
    }
}

/// Writes a document through a temporary file so a crash never leaves half of it.
pub(crate) fn save<T: Document>(path: &Path, doc: &T) -> Result<(), LaunchError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, riven_format::to_string(doc)).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
}

fn config_file(name: &str) -> Result<PathBuf, LaunchError> {
    Ok(riven_sync::config_dir()
        .ok_or(LaunchError::NoDir("config"))?
        .join(name))
}

pub fn settings_path() -> Result<PathBuf, LaunchError> {
    config_file("settings.json")
}

pub fn load_settings() -> Result<Settings, LaunchError> {
    load_or_default(&settings_path()?)
}

pub fn save_settings(settings: &Settings) -> Result<(), LaunchError> {
    save(&settings_path()?, settings)
}
