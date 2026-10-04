use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use riven_format::{InstallSide, Release, State, Trusted};
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
    pub planned: Planned,
    pub groups: BTreeMap<String, bool>,
    pub side: InstallSide,
    origin: String,
    key: Option<String>,
    etag: Option<String>,
    trusted: Trusted,
    trust_path: PathBuf,
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
        let origin = std::fs::canonicalize(&raw).map_err(io)?.display().to_string();
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
        if let Some(s) = &state
            && remote::same_pack(&s.source, &origin)
            && s.etag.as_deref() == Some(digest.as_str())
            && s.side == side
            && request.groups.is_empty()
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
        .filter(|s| remote::same_pack(&s.source, &origin));
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
        planned,
        groups,
        side,
        origin,
        key,
        etag,
        trusted,
        trust_path,
    })))
}

impl Pending {
    /// Downloads and swaps the files in, then records the new state and any newly pinned key.
    pub async fn apply(
        self,
        store: &Store,
        http: &reqwest::Client,
        dir: &Path,
        progress: &(dyn Fn(Event) + Sync),
    ) -> Result<State, SyncError> {
        let files = install::apply(store, http, dir, &self.planned, progress).await?;
        let state = State {
            source: self.origin,
            key: self.key,
            side: self.side,
            version: self.release.version.clone(),
            etag: self.etag,
            groups: self.groups,
            files,
        };
        install::save_state(dir, &state)?;
        trust::save(&self.trust_path, &self.trusted)?;
        Ok(state)
    }
}
