use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use riven_format::{Changes, InstallSide, PackPath, Release, State, Trusted};
use sha2::{Digest as _, Sha256};
use url::Url;

use crate::install::{self, Event, Planned};
use crate::remote::{self, Fetched, Link};
use crate::{Store, SyncError, trust};

pub const DEFAULT_CHANNEL: &str = "stable";

/// What to install into a game directory; unset fields fall back to what is installed there.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// A pack link or a path to a `.riven` archive.
    pub source: Option<String>,
    pub channel: Option<String>,
    pub side: Option<InstallSide>,
    /// Group switches: `x`/`+x` on, `-x` off.
    pub groups: Vec<String>,
    /// `source` is a newer build of the installed pack, even though it lives somewhere else.
    pub replaces: bool,
    /// Pack files the player switched off or removed.
    pub skip: Vec<install::Skip>,
    /// Plans against the disk even when the channel or archive is unchanged.
    pub recheck: bool,
}

pub enum Check {
    /// The channel still points at the installed release.
    UpToDate {
        version: String,
    },
    Ready(Box<Pending>),
}

/// A release planned against a game directory, waiting to be applied.
pub struct Pending {
    pub release: Release,
    manifest_url: Url,
    pub planned: Planned,
    pub groups: BTreeMap<String, bool>,
    pub side: InstallSide,
    origin: String,
    key: Option<String>,
    etag: Option<String>,
    trusted: Trusted,
    trust_path: PathBuf,
    /// What was installed before, when it was this pack.
    previous: Option<State>,
}

fn is_archive(raw: &str) -> bool {
    raw.ends_with(".riven") && Path::new(raw).is_file()
}

/// Reads the release a link or archive points at, to show it before anything is installed.
pub async fn preview(
    store: &Store,
    http: &reqwest::Client,
    source: &str,
    channel: Option<&str>,
) -> Result<Release, SyncError> {
    if is_archive(source) {
        let bytes = std::fs::read(source).map_err(|e| SyncError::Io {
            path: PathBuf::from(source),
            source: e,
        })?;
        return remote::read_archive(&bytes, store);
    }
    let link = remote::parse_link(source, channel.unwrap_or(DEFAULT_CHANNEL))?;
    let mut trusted = trust::load(&trust::default_path()?)?;
    match remote::fetch(http, &link, None, &mut trusted).await? {
        Fetched::Release(remote) => Ok(remote.release),
        Fetched::NotModified => Err(SyncError::NotFound(source.to_owned())),
    }
}

/// Fetches the release the request names and plans it against `dir`; nothing on disk changes.
pub async fn check(
    store: &Store,
    http: &reqwest::Client,
    dir: &Path,
    request: &Request,
) -> Result<Check, SyncError> {
    let state = install::load_state(dir)?;
    let side = request
        .side
        .or(state.as_ref().map(|s| s.side))
        .unwrap_or(InstallSide::Client);
    let raw = request
        .source
        .clone()
        .or_else(|| state.as_ref().map(|s| s.source.clone()))
        .ok_or_else(|| SyncError::NothingInstalled(dir.to_owned()))?;
    let trust_path = trust::default_path()?;
    let mut trusted = trust::load(&trust_path)?;

    let (release, manifest_url, origin, key, etag) = if is_archive(&raw) {
        let io = |source| SyncError::Io {
            path: PathBuf::from(&raw),
            source,
        };
        let bytes = std::fs::read(&raw).map_err(io)?;
        let origin = dunce::canonicalize(&raw).map_err(io)?.display().to_string();
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
        if let Some(s) = &state
            && remote::same_pack(&s.source, &origin)
            && s.etag.as_deref() == Some(digest.as_str())
            && s.side == side
            && request.groups.is_empty()
            && !request.recheck
        {
            return Ok(Check::UpToDate {
                version: s.version.clone(),
            });
        }
        let release = remote::read_archive(&bytes, store)?;
        let url =
            Url::parse("http://riven.invalid/releases/archive.json").expect("static URL parses");
        (release, url, origin, None, Some(digest))
    } else {
        let channel = request.channel.as_deref().unwrap_or(DEFAULT_CHANNEL);
        let link: Link = remote::parse_link(&raw, channel)?;
        let same_pack = state
            .as_ref()
            .is_some_and(|s| s.source == link.pointer.as_str());
        let reuse_etag = same_pack
            && request.groups.is_empty()
            && !request.recheck
            && state.as_ref().is_some_and(|s| s.side == side);
        let etag = state
            .as_ref()
            .filter(|_| reuse_etag)
            .and_then(|s| s.etag.as_deref());
        match remote::fetch(http, &link, etag, &mut trusted).await? {
            Fetched::NotModified => {
                let version = state.map(|s| s.version).unwrap_or_default();
                return Ok(Check::UpToDate { version });
            }
            Fetched::Release(remote) => (
                remote.release,
                remote.manifest_url,
                link.pointer.to_string(),
                remote.key.map(|k| k.to_string()),
                remote.etag,
            ),
        }
    };

    let installed = state
        .as_ref()
        .filter(|s| request.replaces || remote::same_pack(&s.source, &origin));
    let previous = installed.map(|s| s.groups.clone()).unwrap_or_default();
    let groups = install::choose_groups(&release, &previous, &request.groups)?;
    let planned = install::plan(
        dir,
        &release,
        &manifest_url,
        installed,
        &install::Choice {
            side,
            groups: groups.clone(),
            skip: request.skip.clone(),
        },
    )?;
    if let Some(s) = installed
        && s.version == release.version
        && s.side == side
        && s.groups == groups
        && s.etag == etag
        && planned.is_noop()
    {
        return Ok(Check::UpToDate {
            version: s.version.clone(),
        });
    }
    Ok(Check::Ready(Box::new(Pending {
        release,
        manifest_url,
        planned,
        groups,
        side,
        origin,
        key,
        etag,
        trusted,
        trust_path,
        previous: installed.cloned(),
    })))
}

impl Pending {
    /// The release's icon in the store, downloaded if needed.
    pub async fn icon(&self, store: &Store, http: &reqwest::Client) -> Option<PathBuf> {
        let icon = self.release.icon.as_ref()?;
        let urls: Vec<String> = icon
            .urls
            .iter()
            .filter_map(|u| self.manifest_url.join(u).ok().map(String::from))
            .collect();
        match store.fetch(http, &urls, &icon.hashes).await {
            Ok(stored) => Some(stored.path),
            Err(e) => {
                tracing::warn!("cannot fetch the pack icon: {e}");
                None
            }
        }
    }

    /// Whether this moves an installed pack to another version.
    pub fn is_update(&self) -> bool {
        self.previous
            .as_ref()
            .is_some_and(|p| p.version != self.release.version)
    }

    /// What this update changes for the player; a reinstall of the same version keeps the last update's.
    pub fn changes(&self) -> Option<Changes> {
        let previous = self.previous.as_ref()?;
        if previous.version == self.release.version {
            return previous.changes.clone();
        }
        Some(diff(previous, &self.release, &self.planned))
    }

    /// Downloads and swaps the files in, then records the new state and any newly pinned key.
    pub async fn apply(
        self,
        store: &Store,
        http: &reqwest::Client,
        dir: &Path,
        progress: &(dyn Fn(Event) + Sync),
    ) -> Result<State, SyncError> {
        let changes = self.changes();
        let files = install::apply(store, http, dir, &self.planned, progress).await?;
        let state = State {
            source: self.origin,
            key: self.key,
            side: self.side,
            version: self.release.version.clone(),
            etag: self.etag,
            groups: self.groups,
            files,
            changes,
        };
        install::save_state(dir, &state)?;
        trust::save(&self.trust_path, &self.trusted)?;
        Ok(state)
    }
}

/// Folders whose files are content worth naming even without a recorded name.
const CONTENT_FOLDERS: &[&str] = &["mods/", "resourcepacks/", "shaderpacks/"];

/// A file's name for players: the recorded content name, else the file name of content.
fn label(path: &PackPath, name: Option<&String>) -> Option<String> {
    if let Some(name) = name {
        return Some(name.clone());
    }
    let path = path.as_str();
    CONTENT_FOLDERS
        .iter()
        .any(|f| path.starts_with(f))
        .then(|| path.rsplit('/').next().unwrap_or(path).to_owned())
}

/// Compares what `before` had with what `planned` installs and removes, by content name.
fn diff(before: &State, release: &Release, planned: &Planned) -> Changes {
    let removed_names: BTreeSet<String> = planned
        .remove
        .iter()
        .filter_map(|p| label(p, before.files.get(p).and_then(|f| f.name.as_ref())))
        .collect();
    let mut changes = Changes {
        from: before.version.clone(),
        notes: release
            .notes
            .iter()
            .take_while(|n| n.version != before.version)
            .cloned()
            .collect(),
        ..Changes::default()
    };
    let (mut added, mut updated, mut removed) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for file in &planned.install {
        match label(&file.path, file.name.as_ref()) {
            None => changes.other += 1,
            Some(name)
                if before.files.contains_key(&file.path) || removed_names.contains(&name) =>
            {
                updated.insert(name);
            }
            Some(name) => {
                added.insert(name);
            }
        }
    }
    for path in &planned.remove {
        match label(path, before.files.get(path).and_then(|f| f.name.as_ref())) {
            None => changes.other += 1,
            Some(name) if !updated.contains(&name) => {
                removed.insert(name);
            }
            Some(_) => {}
        }
    }
    changes.added = added.into_iter().collect();
    changes.updated = updated.into_iter().collect();
    changes.removed = removed.into_iter().collect();
    changes
}

#[cfg(test)]
mod tests {
    use riven_format::{Hashes, ReleaseNote, StateFile};

    use super::*;
    use crate::install::Wanted;

    fn path(p: &str) -> PackPath {
        PackPath::new(p).unwrap()
    }

    fn wanted(p: &str, name: Option<&str>) -> Wanted {
        Wanted {
            path: path(p),
            name: name.map(str::to_owned),
            hashes: Hashes::default(),
            size: 1,
            urls: Vec::new(),
            preserve: false,
            disabled: false,
        }
    }

    fn before(files: &[(&str, Option<&str>)]) -> State {
        State {
            source: "https://x.test/channels/stable.json".into(),
            key: None,
            side: InstallSide::Client,
            version: "1.0.0".into(),
            etag: None,
            groups: BTreeMap::new(),
            files: files
                .iter()
                .map(|(p, name)| {
                    let record = StateFile {
                        sha512: String::new(),
                        name: name.map(str::to_owned),
                        preserve: false,
                    };
                    (path(p), record)
                })
                .collect(),
            changes: None,
        }
    }

    #[test]
    fn a_renamed_jar_is_an_update_and_configs_are_only_counted() {
        let before = before(&[
            ("mods/sodium-0.5.jar", Some("Sodium")),
            ("mods/jei.jar", Some("JEI")),
            ("mods/old-1.0.jar", None),
            ("config/a.toml", None),
        ]);
        let planned = Planned {
            install: vec![
                wanted("mods/sodium-0.6.jar", Some("Sodium")),
                wanted("mods/iris.jar", Some("Iris")),
                wanted("config/a.toml", None),
            ],
            remove: vec![path("mods/sodium-0.5.jar"), path("mods/old-1.0.jar")],
            ..Planned::default()
        };
        let mut release: Release = riven_format::from_str(
            r#"{ "format": 1, "id": "pack", "name": "Pack", "version": "1.2.0", "minecraft": "1.21.1",
                 "loader": { "type": "neoforge", "version": "21.1.77" }, "java": { "major": 21 } }"#,
        )
        .unwrap();
        release.notes = vec![
            ReleaseNote {
                version: "1.2.0".into(),
                text: "Iris".into(),
            },
            ReleaseNote {
                version: "1.1.0".into(),
                text: "Faster".into(),
            },
            ReleaseNote {
                version: "1.0.0".into(),
                text: "First".into(),
            },
        ];

        let changes = diff(&before, &release, &planned);
        assert_eq!(changes.from, "1.0.0");
        assert_eq!(
            changes
                .notes
                .iter()
                .map(|n| n.version.as_str())
                .collect::<Vec<_>>(),
            ["1.2.0", "1.1.0"]
        );
        assert_eq!(changes.added, ["Iris"]);
        assert_eq!(changes.updated, ["Sodium"]);
        assert_eq!(changes.removed, ["old-1.0.jar"]);
        assert_eq!(changes.other, 1);
    }
}
