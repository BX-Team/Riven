use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use sha1::Sha1;
use sha2::{Digest, Sha512};

use crate::{LaunchError, io};

pub(crate) const DISABLED: &str = ".disabled";

/// A content file in a game directory: a jar in `mods/`, a zip in `resourcepacks/`, …
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentFile {
    pub path: PathBuf,
    /// The file name without a `.disabled` suffix.
    pub name: String,
    pub size: u64,
    pub modified: SystemTime,
    pub enabled: bool,
}

/// What reading a file tells about it: hashes for platform lookups and the jar's own metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Details {
    pub sha512: String,
    pub sha1: String,
    pub title: Option<String>,
    pub version: Option<String>,
}

/// Content files in `dir/<folder>`, newest first; a missing folder is empty.
pub fn scan(game_dir: &Path, folder: &str) -> Result<Vec<ContentFile>, LaunchError> {
    let dir = game_dir.join(folder);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(io(&dir)(e)),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let (name, enabled) = match file_name.strip_suffix(DISABLED) {
            Some(name) => (name.to_owned(), false),
            None => (file_name, true),
        };
        if !(name.ends_with(".jar") || name.ends_with(".zip")) {
            continue;
        }
        out.push(ContentFile {
            path: entry.path(),
            name,
            size: meta.len(),
            modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            enabled,
        });
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.modified));
    Ok(out)
}

/// Hashes a file and reads its mod metadata when it is a jar.
pub fn details(path: &Path) -> Result<Details, LaunchError> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    let meta = riven_resolve::JarMeta::read(&bytes).ok();
    let main = meta.as_ref().and_then(|m| m.mods.first());
    Ok(Details {
        sha512: hex::encode(Sha512::digest(&bytes)),
        sha1: hex::encode(Sha1::digest(&bytes)),
        title: main.and_then(|m| m.name.clone()),
        version: main.map(|m| m.version.to_string()),
    })
}

/// `<game>/.riven/details.json`: what reading each file found, so unchanged files are not read again.
const DETAILS_CACHE: &str = "details.json";

#[derive(Serialize, Deserialize)]
struct Remembered {
    size: u64,
    /// Nanoseconds since the Unix epoch.
    modified: u64,
    details: Details,
}

/// `mods/x.jar`: one cache serves every content folder, so the key carries the folder.
fn cache_key(game_dir: &Path, file: &ContentFile) -> String {
    let folder = file
        .path
        .parent()
        .and_then(|p| p.strip_prefix(game_dir).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    format!("{folder}/{}", file.name)
}

fn stamp(file: &ContentFile) -> u64 {
    file.modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// Details of each file in order, reading only changed ones on a few threads; `progress` gets `(read, to read)`.
pub fn details_cached(
    game_dir: &Path,
    files: &[ContentFile],
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Vec<Option<Details>> {
    let path = riven_sync::install::riven_dir(game_dir).join(DETAILS_CACHE);
    let mut cache: HashMap<String, Remembered> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let keys: Vec<String> = files.iter().map(|f| cache_key(game_dir, f)).collect();
    let mut out: Vec<Option<Details>> = files
        .iter()
        .zip(&keys)
        .map(|(f, key)| {
            cache
                .get(key)
                .filter(|r| r.size == f.size && r.modified == stamp(f))
                .map(|r| r.details.clone())
        })
        .collect();
    let missing: Vec<usize> = (0..files.len()).filter(|&i| out[i].is_none()).collect();
    if missing.is_empty() {
        return out;
    }
    let threads = std::thread::available_parallelism()
        .map_or(2, |n| n.get() / 2)
        .clamp(1, 4);
    let chunk = missing.len().div_ceil(threads);
    let total = missing.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    progress(0, total);
    let read: Vec<(usize, Details)> = std::thread::scope(|scope| {
        let done = &done;
        let workers: Vec<_> = missing
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .filter_map(|&i| {
                            let found = details(&files[i].path);
                            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            progress(n, total);
                            match found {
                                Ok(d) => Some((i, d)),
                                Err(e) => {
                                    tracing::debug!("{e}");
                                    None
                                }
                            }
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect()
    });
    for (i, d) in read {
        out[i] = Some(d);
    }
    let folders: HashSet<&str> = keys
        .iter()
        .filter_map(|k| k.rsplit_once('/').map(|(folder, _)| folder))
        .collect();
    cache.retain(|key, _| {
        key.rsplit_once('/')
            .is_some_and(|(folder, _)| !folders.contains(folder))
    });
    cache.extend(
        files
            .iter()
            .zip(keys)
            .zip(&out)
            .filter_map(|((f, key), d)| {
                Some((
                    key,
                    Remembered {
                        size: f.size,
                        modified: stamp(f),
                        details: d.clone()?,
                    },
                ))
            }),
    );
    if let Ok(text) = serde_json::to_string(&cache) {
        let _ = std::fs::create_dir_all(path.parent().expect("cache path has a parent"));
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    out
}

/// Turns a file on or off the way other launchers do, with a `.disabled` suffix.
pub fn set_enabled(file: &ContentFile, enabled: bool) -> Result<PathBuf, LaunchError> {
    if file.enabled == enabled {
        return Ok(file.path.clone());
    }
    let dir = file.path.parent().expect("content files live in a folder");
    let target = if enabled {
        dir.join(&file.name)
    } else {
        dir.join(format!("{}{DISABLED}", file.name))
    };
    std::fs::rename(&file.path, &target).map_err(io(&file.path))?;
    Ok(target)
}

pub fn delete(file: &ContentFile) -> Result<(), LaunchError> {
    std::fs::remove_file(&file.path).map_err(io(&file.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_details_follow_file_changes() {
        let game = std::env::temp_dir().join(format!("riven-details-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&game);
        std::fs::create_dir_all(game.join("mods")).unwrap();
        let jar = game.join("mods/a.jar");
        let read = || {
            let files = scan(&game, "mods").unwrap();
            details_cached(&game, &files, &|_, _| {})
                .remove(0)
                .unwrap()
                .sha512
        };
        std::fs::write(&jar, "one").unwrap();
        let first = read();
        assert!(game.join(".riven").join(DETAILS_CACHE).is_file());
        assert_eq!(read(), first);
        std::fs::create_dir_all(game.join("resourcepacks")).unwrap();
        std::fs::write(game.join("resourcepacks/p.zip"), "pack").unwrap();
        let packs = scan(&game, "resourcepacks").unwrap();
        details_cached(&game, &packs, &|_, _| {});
        let mods = scan(&game, "mods").unwrap();
        let rereads = std::sync::atomic::AtomicUsize::new(0);
        details_cached(&game, &mods, &|_, _| {
            rereads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });
        assert_eq!(
            rereads.into_inner(),
            0,
            "reading another folder kept the mods cached"
        );
        std::fs::write(&jar, "other").unwrap();
        assert_ne!(read(), first);
        let _ = std::fs::remove_dir_all(&game);
    }
}
