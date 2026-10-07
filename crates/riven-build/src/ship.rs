use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use futures_util::{StreamExt, stream};
use riven_format::{Channel, Entry, InstallSide, KeyPair, Release, SignError};
use riven_resolve::JarFetcher;

use crate::author::AuthorError;
use crate::deploy::{self, DeployError, Deployed};
use crate::export::{self, Exported, Format, needs_bytes, selected};
use crate::release::{self, Built};
use crate::workspace::{StoreJars, Workspace, ensure_gitignore};

const PARALLEL_DOWNLOADS: usize = 8;
pub const DIST: &str = "dist";
pub const EXPORTS: &str = "exports";
pub const DEFAULT_BRANCH: &str = "gh-pages";
pub const DEFAULT_REMOTE: &str = "origin";

#[derive(Debug, thiserror::Error)]
pub enum ShipError {
    #[error(transparent)]
    Author(#[from] AuthorError),
    #[error(transparent)]
    Release(#[from] release::Error),
    #[error(transparent)]
    Deploy(#[from] DeployError),
    #[error("cannot locate the config directory")]
    NoConfigDir,
    #[error("{}: {source}", path.display())]
    Key { path: PathBuf, source: SignError },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("no signing key for `{0}`; create one with `riven keygen`, or build unsigned")]
    NoKey(String),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ShipError + '_ {
    move |source| ShipError::Io {
        path: path.to_owned(),
        source,
    }
}

/// `<config>/riven/keys/<pack>.ed25519`.
pub fn key_path(pack: &str) -> Result<PathBuf, ShipError> {
    let dir = riven_sync::config_dir().ok_or(ShipError::NoConfigDir)?;
    Ok(dir.join("keys").join(format!("{pack}.ed25519")))
}

pub fn load_key(pack: &str) -> Result<Option<KeyPair>, ShipError> {
    let path = key_path(pack)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => KeyPair::from_secret(&text)
            .map(Some)
            .map_err(|source| ShipError::Key { path, source }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io(&path)(e)),
    }
}

fn write_secret(path: &Path, text: &str) -> Result<(), ShipError> {
    let parent = path.parent().expect("key path has a parent");
    std::fs::create_dir_all(parent).map_err(io(parent))?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(io(path))?;
        file.write_all(text.as_bytes()).map_err(io(path))?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, text).map_err(io(path))?;
    Ok(())
}

/// The pack's signing key, created on first use; `true` when it was just created.
pub fn keygen(pack: &str) -> Result<(KeyPair, PathBuf, bool), ShipError> {
    let path = key_path(pack)?;
    if let Some(key) = load_key(pack)? {
        return Ok((key, path, false));
    }
    let key = KeyPair::generate().map_err(|source| ShipError::Key {
        path: path.clone(),
        source,
    })?;
    write_secret(&path, &key.to_secret())?;
    Ok((key, path, true))
}

/// The signing key a build uses: required unless `unsigned`.
pub fn signing_key(pack: &str, unsigned: bool) -> Result<Option<KeyPair>, ShipError> {
    if unsigned {
        return Ok(None);
    }
    load_key(pack)?
        .map(Some)
        .ok_or_else(|| ShipError::NoKey(pack.to_owned()))
}

/// Builds the current version into `dist`, then points `channel` at it.
pub fn build(
    ws: &Workspace,
    dist: &Path,
    channel: &str,
    key: Option<&KeyPair>,
) -> Result<(Built, Channel), ShipError> {
    let built = release::build_release(&ws.project, &ws.dir)?;
    release::write_release(dist, &built, key)?;
    let pointer = release::publish(dist, channel, &ws.project.version, key)?;
    Ok((built, pointer))
}

/// Writes the project as it is now to `.riven/test/<id>.riven`, unsigned, for a test instance.
pub fn test_archive(ws: &Workspace) -> Result<PathBuf, ShipError> {
    let dir = ws.dir.join(".riven").join("test");
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let path = dir.join(format!("{}.riven", ws.project.id));
    let built = release::build_release(&ws.project, &ws.dir)?;
    release::write_archive(&built, None, &path)?;
    Ok(path)
}

/// Compares dotted versions part by part, numbers numerically.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let mut left = a.split(['.', '-', '+']);
    let mut right = b.split(['.', '-', '+']);
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(y)) => return prerelease(y).reverse(),
            (Some(x), None) => return prerelease(x),
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    _ => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// A trailing part after an equal prefix: a number makes a version newer, a tag (`beta`) older.
fn prerelease(part: &str) -> Ordering {
    if part.parse::<u64>().is_ok() {
        Ordering::Greater
    } else {
        Ordering::Less
    }
}

/// Versions built into `dist/releases/`, newest first.
pub fn releases(dist: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dist.join("releases")) else {
        return Vec::new();
    };
    let mut versions: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_owned)
        })
        .collect();
    versions.sort_by(|a, b| compare_versions(b, a));
    versions
}

pub fn read_release(dist: &Path, version: &str) -> Option<Release> {
    let text =
        std::fs::read_to_string(dist.join("releases").join(format!("{version}.json"))).ok()?;
    riven_format::from_str(&text).ok()
}

/// Channel pointers in `dist/channels/`, by name.
pub fn channels(dist: &Path) -> BTreeMap<String, Channel> {
    let Ok(entries) = std::fs::read_dir(dist.join("channels")) else {
        return BTreeMap::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.strip_suffix(".json")?.to_owned();
            let text = std::fs::read_to_string(e.path()).ok()?;
            Some((name, riven_format::from_str(&text).ok()?))
        })
        .collect()
}

/// How a file differs between two releases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Added(String),
    Removed(String),
    Changed(String),
}

/// What changed between releases, by installed path, sorted.
pub fn changes(old: &Release, new: &Release) -> Vec<FileChange> {
    let before: BTreeMap<&str, Option<&str>> = old
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.hashes.sha512.as_deref()))
        .collect();
    let after: BTreeMap<&str, Option<&str>> = new
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.hashes.sha512.as_deref()))
        .collect();
    let mut out: Vec<FileChange> = Vec::new();
    for (path, hash) in &after {
        match before.get(path) {
            None => out.push(FileChange::Added((*path).to_owned())),
            Some(h) if h != hash => out.push(FileChange::Changed((*path).to_owned())),
            Some(_) => {}
        }
    }
    out.extend(
        before
            .keys()
            .filter(|p| !after.contains_key(*p))
            .map(|p| FileChange::Removed((*p).to_owned())),
    );
    out.sort_by(|a, b| change_path(a).cmp(change_path(b)));
    out
}

fn change_path(change: &FileChange) -> &str {
    match change {
        FileChange::Added(p) | FileChange::Removed(p) | FileChange::Changed(p) => p,
    }
}

/// A deploy, plus the install link it gives players.
pub struct Shipped {
    pub deployed: Deployed,
    pub link: Option<String>,
}

/// Commits `dist` to `branch` and pushes it when asked.
pub fn deploy(
    ws: &Workspace,
    dist: &Path,
    branch: &str,
    remote: &str,
    push: bool,
) -> Result<Shipped, ShipError> {
    let message = format!("deploy {} {}", ws.project.id, ws.project.version);
    let options = deploy::Deploy {
        dist,
        branch,
        remote,
        message: &message,
        push,
    };
    let deployed = deploy::deploy(&ws.dir, &options)?;
    let key = load_key(&ws.project.id)?.map(|k| k.public().to_string());
    let link = deployed.short_link.as_ref().map(|base| match &key {
        Some(key) => format!("{base}#key={key}"),
        None => base.clone(),
    });
    Ok(Shipped { deployed, link })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Mrpack,
    Prism,
    Riven,
}

/// Where an export lands by default: `exports/<id>-<version>[-prism].<ext>`.
pub fn export_path(ws: &Workspace, kind: ExportKind) -> PathBuf {
    let (suffix, ext) = match kind {
        ExportKind::Mrpack => ("", Format::Mrpack.extension()),
        ExportKind::Prism => ("-prism", Format::Prism.extension()),
        ExportKind::Riven => ("", "riven"),
    };
    ws.dir.join(EXPORTS).join(format!(
        "{}-{}{suffix}.{ext}",
        ws.project.id, ws.project.version
    ))
}

#[derive(Debug, Default)]
pub struct ExportReport {
    pub path: PathBuf,
    pub exported: Exported,
    /// Files that could not be fetched to embed, as `id: reason`.
    pub failures: Vec<String>,
}

async fn fetch(jars: &StoreJars, entry: &Entry) -> Option<String> {
    jars.jar(entry)
        .await
        .err()
        .map(|e| format!("`{}`: {e}", entry.id))
}

/// Exports the pack, fetching the files the format must embed first.
pub async fn export(
    ws: &Workspace,
    kind: ExportKind,
    side: Option<InstallSide>,
    path: Option<PathBuf>,
) -> Result<ExportReport, ShipError> {
    ensure_gitignore(&ws.dir)?;
    let path = path.unwrap_or_else(|| export_path(ws, kind));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let format = match kind {
        ExportKind::Riven => {
            let key = load_key(&ws.project.id)?;
            let built = release::build_release(&ws.project, &ws.dir)?;
            release::write_archive(&built, key.as_ref(), &path)?;
            return Ok(ExportReport {
                path,
                exported: Exported {
                    linked: built.release.files.len() - built.blobs.len(),
                    embedded: built.blobs.len(),
                    skipped: Vec::new(),
                },
                failures: Vec::new(),
            });
        }
        ExportKind::Mrpack => Format::Mrpack,
        ExportKind::Prism => Format::Prism,
    };
    let jars = ws.jars()?;
    let wanted: Vec<&Entry> = selected(format, &ws.project, side)
        .into_iter()
        .filter(|e| needs_bytes(format, e))
        .collect();
    let fetches: Vec<_> = wanted.into_iter().map(|e| fetch(&jars, e)).collect();
    let failures: Vec<String> = stream::iter(fetches)
        .buffer_unordered(PARALLEL_DOWNLOADS)
        .filter_map(|e| async move { e })
        .collect()
        .await;
    let bytes = |entry: &Entry| -> Result<Vec<u8>, String> {
        let file = jars
            .local_path(entry)
            .ok_or_else(|| "not downloaded".to_owned())?;
        std::fs::read(&file).map_err(|e| e.to_string())
    };
    let exported = export::export(format, &ws.project, &ws.dir, side, &bytes, &path)?;
    Ok(ExportReport {
        path,
        exported,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_sort_numerically() {
        let mut v = vec!["1.10.0", "1.9.2", "1.9.10", "2.0.0-beta", "2.0.0"];
        v.sort_by(|a, b| compare_versions(b, a));
        assert_eq!(v, ["2.0.0", "2.0.0-beta", "1.10.0", "1.9.10", "1.9.2"]);
    }
}
