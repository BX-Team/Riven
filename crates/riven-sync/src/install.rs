use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{StreamExt, stream};
use riven_format::{Hash, Hashes, InstallSide, PackPath, Release, State, StateFile};
use sha1::Sha1;
use sha2::{Digest, Sha512};
use url::Url;

use crate::{Store, SyncError};

const PARALLEL_DOWNLOADS: usize = 8;
const ATTEMPTS: u32 = 3;

/// Where an instance keeps riven's bookkeeping.
pub fn riven_dir(dir: &Path) -> PathBuf {
    dir.join(".riven")
}

pub fn state_path(dir: &Path) -> PathBuf {
    riven_dir(dir).join("state.json")
}

pub fn load_state(dir: &Path) -> Result<Option<State>, SyncError> {
    let path = state_path(dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(riven_format::from_str(&text)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SyncError::Io { path, source }),
    }
}

/// Written last, so an interrupted install leaves the previous state to resume from.
pub fn save_state(dir: &Path, state: &State) -> Result<(), SyncError> {
    let path = state_path(dir);
    let io = |source| SyncError::Io {
        path: path.clone(),
        source,
    };
    std::fs::create_dir_all(riven_dir(dir)).map_err(io)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, riven_format::to_string(state)).map_err(io)?;
    std::fs::rename(&tmp, &path).map_err(io)
}

/// Group switches: release defaults, then earlier choices, then `requested` (`x`/`+x` on, `-x` off).
pub fn choose_groups(
    release: &Release,
    previous: &BTreeMap<String, bool>,
    requested: &[String],
) -> Result<BTreeMap<String, bool>, SyncError> {
    let mut groups: BTreeMap<String, bool> = release
        .groups
        .iter()
        .map(|g| {
            (
                g.id.clone(),
                previous.get(&g.id).copied().unwrap_or(g.default),
            )
        })
        .collect();
    for raw in requested.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
        let (name, on) = match raw.strip_prefix('-') {
            Some(name) => (name, false),
            None => (raw.trim_start_matches('+'), true),
        };
        match groups.get_mut(name) {
            Some(slot) => *slot = on,
            None => return Err(SyncError::UnknownGroup(name.to_owned())),
        }
    }
    Ok(groups)
}

/// A file the release wants on disk.
#[derive(Debug, Clone)]
pub struct Wanted {
    pub path: PackPath,
    pub hashes: Hashes,
    pub size: u64,
    /// Absolute mirrors.
    pub urls: Vec<String>,
    pub preserve: bool,
}

/// What an install will do.
#[derive(Debug, Default)]
pub struct Planned {
    pub install: Vec<Wanted>,
    /// Files already in place, with their state records.
    pub unchanged: Vec<(PackPath, StateFile)>,
    /// Preserved files the user changed; left alone.
    pub kept: Vec<(PackPath, Option<StateFile>)>,
    /// Files riven installed earlier that the release no longer has.
    pub remove: Vec<PackPath>,
}

impl Planned {
    pub fn is_noop(&self) -> bool {
        self.install.is_empty() && self.remove.is_empty()
    }
}

struct Digests {
    sha512: String,
    sha1: String,
}

fn digest_file(path: &Path) -> Result<Digests, SyncError> {
    let mut file = std::fs::File::open(path).map_err(|source| SyncError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut sha512 = Sha512::new();
    let mut sha1 = Sha1::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|source| SyncError::Io {
            path: path.to_owned(),
            source,
        })?;
        if n == 0 {
            break;
        }
        sha512.update(&buf[..n]);
        sha1.update(&buf[..n]);
    }
    Ok(Digests {
        sha512: hex::encode(sha512.finalize()),
        sha1: hex::encode(sha1.finalize()),
    })
}

fn matches(hashes: &Hashes, disk: &Digests) -> bool {
    match hashes.strongest() {
        Some(Hash::Sha512(h)) => h.eq_ignore_ascii_case(&disk.sha512),
        Some(Hash::Sha1(h)) => h.eq_ignore_ascii_case(&disk.sha1),
        _ => false,
    }
}

/// Refuses to write through a symlink anywhere between `dir` and `path`.
pub fn safe_target(dir: &Path, path: &PackPath) -> Result<PathBuf, SyncError> {
    let mut current = dir.to_owned();
    for part in path.as_str().split('/') {
        current.push(part);
        if std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(SyncError::Symlink(current));
        }
    }
    Ok(current)
}

fn included(
    side: riven_format::Side,
    group: Option<&String>,
    install: InstallSide,
    groups: &BTreeMap<String, bool>,
) -> bool {
    side.includes(install) && group.is_none_or(|g| groups.get(g).copied().unwrap_or(false))
}

/// Which part of a release an instance gets.
#[derive(Debug, Clone)]
pub struct Choice {
    pub side: InstallSide,
    pub groups: BTreeMap<String, bool>,
}

/// Compares what the release wants with `state` and the disk.
pub fn plan(
    dir: &Path,
    release: &Release,
    manifest_url: &Url,
    state: Option<&State>,
    choice: &Choice,
) -> Result<Planned, SyncError> {
    let (side, groups) = (choice.side, &choice.groups);
    let empty = BTreeMap::new();
    let previous = state.map_or(&empty, |s| &s.files);
    let mut planned = Planned::default();
    let mut wanted: Vec<Wanted> = Vec::new();

    for file in &release.files {
        if !included(file.side, file.group.as_ref(), side, groups) {
            continue;
        }
        let urls = file
            .urls
            .iter()
            .map(|u| manifest_url.join(u).map(String::from))
            .collect::<Result<_, _>>()
            .map_err(|_| SyncError::BadLink(file.urls.join(" ")))?;
        wanted.push(Wanted {
            path: file.path.clone(),
            hashes: file.hashes.clone(),
            size: file.size,
            urls,
            preserve: file.preserve,
        });
    }
    let mut seen = HashSet::new();
    for file in wanted {
        if !seen.insert(file.path.clone()) {
            return Err(SyncError::Conflict(file.path));
        }
        let target = safe_target(dir, &file.path)?;
        let prev = previous.get(&file.path);
        if !target.exists() {
            planned.install.push(file);
            continue;
        }
        let same_size = std::fs::metadata(&target).is_ok_and(|m| m.len() == file.size);
        let recorded = prev.is_some_and(|p| file.hashes.sha512.as_ref() == Some(&p.sha512));
        if recorded && same_size {
            let record = prev.cloned().expect("recorded implies a previous record");
            planned.unchanged.push((
                file.path,
                StateFile {
                    preserve: file.preserve,
                    ..record
                },
            ));
            continue;
        }
        let disk = digest_file(&target)?;
        if matches(&file.hashes, &disk) {
            let record = StateFile {
                sha512: disk.sha512,
                preserve: file.preserve,
            };
            planned.unchanged.push((file.path, record));
        } else if file.preserve && prev.is_none_or(|p| p.sha512 != disk.sha512) {
            planned.kept.push((file.path, prev.cloned()));
        } else {
            planned.install.push(file);
        }
    }

    for (path, prev) in previous {
        if seen.contains(path) {
            continue;
        }
        let target = safe_target(dir, path)?;
        if !target.exists() {
            continue;
        }
        if prev.preserve && digest_file(&target)?.sha512 != prev.sha512 {
            continue;
        }
        planned.remove.push(path.clone());
    }
    Ok(planned)
}

/// Progress of [`apply`], for CLI bars and GUI views alike.
#[derive(Debug, Clone, Copy)]
pub enum Event {
    Downloading { done: usize, total: usize },
    Installing { done: usize, total: usize },
}

async fn fetch_with_retries(
    store: &Store,
    http: &reqwest::Client,
    file: &Wanted,
) -> Result<crate::Stored, SyncError> {
    let mut delay = Duration::from_millis(500);
    let mut attempt = 1;
    loop {
        match store.fetch(http, &file.urls, &file.hashes).await {
            Ok(stored) => return Ok(stored),
            Err(e @ crate::Error::HashMismatch { .. }) => return Err(e.into()),
            Err(e) if attempt >= ATTEMPTS => return Err(e.into()),
            Err(e) => {
                tracing::warn!("{}: {e}; retrying", file.path);
                tokio::time::sleep(delay).await;
                delay *= 2;
                attempt += 1;
            }
        }
    }
}

/// Jars and pack zips are never edited in place, so they may share the store's inode.
fn may_hardlink(path: &PackPath) -> bool {
    let p = path.as_str();
    matches!(
        p.split('/').next(),
        Some("mods" | "resourcepacks" | "shaderpacks")
    ) && (p.ends_with(".jar") || p.ends_with(".zip"))
}

fn place(stored: &Path, staging: &Path, target: &Path, link: bool) -> Result<(), SyncError> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| SyncError::Io { path, source }
    };
    let _ = std::fs::remove_file(staging);
    let linked = link && std::fs::hard_link(stored, staging).is_ok();
    if !linked {
        std::fs::copy(stored, staging).map_err(io(staging))?;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    std::fs::rename(staging, target).map_err(io(target))
}

/// Downloads into the store, swaps files in through a staging directory and removes dropped ones.
pub async fn apply(
    store: &Store,
    http: &reqwest::Client,
    dir: &Path,
    planned: &Planned,
    progress: &(dyn Fn(Event) + Sync),
) -> Result<BTreeMap<PackPath, StateFile>, SyncError> {
    let total = planned.install.len();
    let mut done = 0;
    progress(Event::Downloading { done, total });
    let mut fetches = stream::iter(&planned.install)
        .map(|file| async move { (file, fetch_with_retries(store, http, file).await) })
        .buffer_unordered(PARALLEL_DOWNLOADS);
    let mut stored = Vec::with_capacity(total);
    while let Some((file, result)) = fetches.next().await {
        stored.push((file, result?));
        done += 1;
        progress(Event::Downloading { done, total });
    }

    let staging = riven_dir(dir).join("staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|source| SyncError::Io {
        path: staging.clone(),
        source,
    })?;
    let mut files: BTreeMap<PackPath, StateFile> = planned.unchanged.iter().cloned().collect();
    for (path, prev) in &planned.kept {
        if let Some(prev) = prev {
            files.insert(path.clone(), prev.clone());
        }
    }
    for (n, (file, blob)) in stored.into_iter().enumerate() {
        let target = safe_target(dir, &file.path)?;
        let temp = staging.join(n.to_string());
        place(&blob.path, &temp, &target, may_hardlink(&file.path))?;
        files.insert(
            file.path.clone(),
            StateFile {
                sha512: blob.sha512,
                preserve: file.preserve,
            },
        );
        progress(Event::Installing { done: n + 1, total });
    }
    for path in &planned.remove {
        let target = safe_target(dir, path)?;
        if let Err(source) = std::fs::remove_file(&target)
            && source.kind() != std::io::ErrorKind::NotFound
        {
            return Err(SyncError::Io {
                path: target,
                source,
            });
        }
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(files)
}

#[cfg(test)]
mod tests {
    use riven_format::{Group, Java, Loader, LoaderKind, ReleaseFile, Side};

    use super::*;

    struct Fixture {
        root: PathBuf,
        store: Store,
        dir: PathBuf,
        url: Url,
        http: reqwest::Client,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("riven-install-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let dir = root.join("instance");
            std::fs::create_dir_all(&dir).unwrap();
            Self {
                store: Store::new(root.join("store")),
                dir,
                root,
                url: Url::parse("http://127.0.0.1:9/releases/1.json").unwrap(),
                http: reqwest::Client::new(),
            }
        }

        /// A release file whose bytes are already in the store, so nothing is downloaded.
        fn file(&self, path: &str, body: &str, side: Side, preserve: bool) -> ReleaseFile {
            let stored = self
                .store
                .insert(body.as_bytes(), &Hashes::default(), "test")
                .unwrap();
            ReleaseFile {
                path: PackPath::new(path).unwrap(),
                size: body.len() as u64,
                hashes: Hashes {
                    sha512: Some(stored.sha512),
                    ..Hashes::default()
                },
                urls: vec!["../blobs/unused".into()],
                side,
                group: None,
                preserve,
            }
        }

        async fn install(&self, release: &Release) -> Planned {
            let state = load_state(&self.dir).unwrap();
            let groups = choose_groups(release, &BTreeMap::new(), &[]).unwrap();
            let choice = Choice {
                side: InstallSide::Client,
                groups: groups.clone(),
            };
            let planned = plan(&self.dir, release, &self.url, state.as_ref(), &choice).unwrap();
            let files = apply(&self.store, &self.http, &self.dir, &planned, &|_| {})
                .await
                .unwrap();
            save_state(
                &self.dir,
                &State {
                    source: self.url.to_string(),
                    key: None,
                    side: InstallSide::Client,
                    version: release.version.clone(),
                    etag: None,
                    groups,
                    files,
                },
            )
            .unwrap();
            planned
        }

        fn read(&self, path: &str) -> String {
            std::fs::read_to_string(self.dir.join(path)).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn release(version: &str, files: Vec<ReleaseFile>) -> Release {
        Release {
            id: "pack".into(),
            name: "Pack".into(),
            version: version.into(),
            minecraft: "1.21.1".into(),
            loader: Loader {
                kind: LoaderKind::NeoForge,
                version: "21.1.77".into(),
            },
            java: Java {
                major: 21,
                memory: None,
                jvm_args: vec![],
            },
            groups: vec![Group {
                id: "shaders".into(),
                name: "Shaders".into(),
                description: None,
                default: false,
            }],
            files,
        }
    }

    #[tokio::test]
    async fn installs_side_files_and_reruns_as_noop() {
        let fx = Fixture::new("noop");
        let mut shader = fx.file("shaderpacks/bsl.zip", "bsl", Side::Client, false);
        shader.group = Some("shaders".into());
        let v1 = release(
            "1",
            vec![
                fx.file("mods/a.jar", "a1", Side::Both, false),
                fx.file("mods/server.jar", "s", Side::Server, false),
                shader,
            ],
        );
        let first = fx.install(&v1).await;
        assert_eq!(first.install.len(), 1);
        assert_eq!(fx.read("mods/a.jar"), "a1");
        assert!(!fx.dir.join("mods/server.jar").exists());
        assert!(!fx.dir.join("shaderpacks/bsl.zip").exists());
        let again = fx.install(&v1).await;
        assert!(again.is_noop());
        assert_eq!(again.unchanged.len(), 1);
    }

    #[tokio::test]
    async fn preserve_keeps_user_edits_and_removal_spares_user_files() {
        let fx = Fixture::new("preserve");
        let v1 = release(
            "1",
            vec![
                fx.file("options.txt", "fov:70", Side::Client, true),
                fx.file("config/a.toml", "a=1", Side::Both, true),
                fx.file("mods/old.jar", "old", Side::Both, false),
            ],
        );
        fx.install(&v1).await;
        std::fs::write(fx.dir.join("options.txt"), "fov:110").unwrap();
        std::fs::write(fx.dir.join("mods/mine.jar"), "player's mod").unwrap();

        let v2 = release(
            "2",
            vec![
                fx.file("options.txt", "fov:80", Side::Client, true),
                fx.file("config/a.toml", "a=2", Side::Both, true),
            ],
        );
        let planned = fx.install(&v2).await;
        assert_eq!(fx.read("options.txt"), "fov:110");
        assert_eq!(fx.read("config/a.toml"), "a=2");
        assert!(!fx.dir.join("mods/old.jar").exists());
        assert_eq!(fx.read("mods/mine.jar"), "player's mod");
        assert_eq!(planned.kept.len(), 1);

        // Still the user's file on the next release, even though riven recorded it once.
        let v3 = release(
            "3",
            vec![fx.file("options.txt", "fov:90", Side::Client, true)],
        );
        fx.install(&v3).await;
        assert_eq!(fx.read("options.txt"), "fov:110");
    }

    #[tokio::test]
    async fn interrupted_install_resumes_without_mistaking_new_files_for_edits() {
        let fx = Fixture::new("resume");
        let v1 = release(
            "1",
            vec![
                fx.file("options.txt", "fov:70", Side::Client, true),
                fx.file("mods/a.jar", "a1", Side::Both, false),
            ],
        );
        fx.install(&v1).await;
        let v2 = release(
            "2",
            vec![
                fx.file("options.txt", "fov:80", Side::Client, true),
                fx.file("mods/a.jar", "a2", Side::Both, false),
            ],
        );
        // The crash happened after options.txt was swapped in but before state.json was written.
        std::fs::write(fx.dir.join("options.txt"), "fov:80").unwrap();
        let planned = fx.install(&v2).await;
        assert!(planned.kept.is_empty());
        assert_eq!(planned.install.len(), 1);
        assert_eq!(fx.read("mods/a.jar"), "a2");
        let state = load_state(&fx.dir).unwrap().unwrap();
        assert_eq!(state.files.len(), 2);
        assert!(fx.install(&v2).await.is_noop());
    }

    #[test]
    fn group_choices_layer_defaults_previous_and_requested() {
        let r = release("1", vec![]);
        let none = BTreeMap::new();
        assert!(!choose_groups(&r, &none, &[]).unwrap()["shaders"]);
        let on = choose_groups(&r, &none, &["+shaders".into()]).unwrap();
        assert!(on["shaders"]);
        assert!(choose_groups(&r, &on, &[]).unwrap()["shaders"]);
        assert!(!choose_groups(&r, &on, &["-shaders".into()]).unwrap()["shaders"]);
        assert!(matches!(
            choose_groups(&r, &none, &["minimap".into()]),
            Err(SyncError::UnknownGroup(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_write_through_symlinks() {
        let fx = Fixture::new("symlink");
        let outside = fx.root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, fx.dir.join("mods")).unwrap();
        let path = PackPath::new("mods/evil.jar").unwrap();
        assert!(matches!(
            safe_target(&fx.dir, &path),
            Err(SyncError::Symlink(_))
        ));
    }
}
