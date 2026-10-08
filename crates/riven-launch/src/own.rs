use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use riven_format::{
    Entry, EntryFile, Hashes, Instance, Java, Kind, OwnContent, PackPath, Project, Reason, Release,
    Side, Source, StateFile, UpdatePolicy,
};
use riven_resolve::{AddOptions, Downloaded, JarFetcher, Plan, Planner};
use riven_sources::{Cache, Known, Modrinth};
use riven_sync::{Store, install};

use crate::instances::Instances;
use crate::mods::{self, DISABLED};
use crate::{LaunchError, io};

const API_CACHE_TTL: Duration = Duration::from_secs(3600);

pub fn own_path(game_dir: &Path) -> PathBuf {
    install::riven_dir(game_dir).join("own.json")
}

pub fn load(game_dir: &Path) -> Result<OwnContent, LaunchError> {
    crate::load_or_default(&own_path(game_dir))
}

pub fn save(game_dir: &Path, own: &OwnContent) -> Result<(), LaunchError> {
    crate::save(&own_path(game_dir), own)
}

/// Whether an instance came from a pack, which then decides its mods.
pub fn from_pack(game_dir: &Path) -> bool {
    install::state_path(game_dir).is_file()
}

/// The player may change mods in their own instances, and in pack ones once they unlocked them.
pub fn editable(instance: &Instance, game_dir: &Path) -> bool {
    instance.own_mods || !from_pack(game_dir)
}

/// The on-disk file of an entry, enabled or not.
fn on_disk(game_dir: &Path, path: &PackPath) -> Result<Option<PathBuf>, LaunchError> {
    let target = install::safe_target(game_dir, path)?;
    let disabled = PathBuf::from(format!("{}{DISABLED}", target.display()));
    Ok([target, disabled].into_iter().find(|p| p.is_file()))
}

fn delete(game_dir: &Path, path: &PackPath) -> Result<(), LaunchError> {
    if let Some(file) = on_disk(game_dir, path)? {
        std::fs::remove_file(&file).map_err(io(&file))?;
    }
    Ok(())
}

fn slug(raw: &str) -> String {
    let lowered: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug: Vec<&str> = lowered.split('-').filter(|s| !s.is_empty()).collect();
    if slug.is_empty() {
        "file".into()
    } else {
        slug.join("-")
    }
}

/// Reads jars from the store, then from the game directory, then downloads them.
struct GameJars {
    store: Store,
    http: reqwest::Client,
    /// sha512 → a file in `mods/` with those bytes.
    disk: HashMap<String, PathBuf>,
}

impl GameJars {
    async fn stored(&self, file: &EntryFile) -> Result<PathBuf, String> {
        if let Some(path) = self.store.get(&file.hashes) {
            return Ok(path);
        }
        let urls: Vec<String> = file.url.iter().cloned().collect();
        self.store
            .fetch(&self.http, &urls, &file.hashes)
            .await
            .map(|s| s.path)
            .map_err(|e| e.to_string())
    }

    fn keep(&self, bytes: Vec<u8>, origin: &str) -> Result<Downloaded, LaunchError> {
        let stored = self.store.insert(&bytes, &Hashes::default(), origin)?;
        Ok(Downloaded {
            bytes,
            hashes: Hashes {
                sha512: Some(stored.sha512),
                sha1: Some(stored.sha1),
            },
            size: stored.size,
        })
    }
}

impl JarFetcher for GameJars {
    async fn jar(&self, entry: &Entry) -> Result<Vec<u8>, String> {
        let local = entry
            .file
            .hashes
            .sha512
            .as_ref()
            .and_then(|h| self.disk.get(h))
            .cloned();
        let path = match (self.store.get(&entry.file.hashes), local) {
            (Some(path), _) | (None, Some(path)) => path,
            (None, None) => self.stored(&entry.file).await?,
        };
        tokio::fs::read(&path)
            .await
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// An instance's mods as the resolver sees them: the player's entries and every other jar on disk.
pub struct Workbench {
    game_dir: PathBuf,
    project: Project,
    /// Mods stay the pack's until the player allows their own; packs and shaders never lock.
    mods_locked: bool,
    /// No loader: resource packs and shaders only, under a stand-in loader nothing filters by.
    vanilla: bool,
    own: OwnContent,
    modrinth: Modrinth,
    jars: GameJars,
}

impl Workbench {
    /// Reads the instance's mods and looks the ones the player did not add up on Modrinth.
    pub async fn open(store: &Instances, id: &str) -> Result<Self, LaunchError> {
        Self::read(store, id, true).await
    }

    /// Only the player's own entries: enough to remove them, not to resolve new dependencies.
    pub async fn open_own(store: &Instances, id: &str) -> Result<Self, LaunchError> {
        Self::read(store, id, false).await
    }

    async fn read(store: &Instances, id: &str, others: bool) -> Result<Self, LaunchError> {
        let instance = store.load(id)?;
        let game_dir = store.game_dir(id);
        let mods_locked = !editable(&instance, &game_dir);
        let vanilla = instance.loader.is_none();
        let loader = instance.loader.clone().unwrap_or(riven_format::Loader {
            kind: riven_format::LoaderKind::Fabric,
            version: String::new(),
        });
        let own = load(&game_dir)?;
        let data = riven_sync::data_dir().ok_or(LaunchError::NoDir("data"))?;
        let modrinth = Modrinth::new(riven_sources::client())
            .with_cache(Cache::new(data.join("cache").join("api"), API_CACHE_TTL));

        let owned: HashSet<String> = own
            .content
            .iter()
            .map(|e| e.file.path.file_name().to_owned())
            .collect();
        let scan_dir = game_dir.clone();
        let others = tokio::task::spawn_blocking(move || -> Result<_, LaunchError> {
            let mut out = Vec::new();
            if !others || vanilla {
                return Ok(out);
            }
            let files: Vec<_> = mods::scan(&scan_dir, "mods")?
                .into_iter()
                .filter(|f| !owned.contains(&f.name))
                .collect();
            let found = mods::details_cached(&scan_dir, &files, &|_, _| {});
            out.extend(
                files
                    .into_iter()
                    .zip(found)
                    .filter_map(|(file, details)| Some((file, details?))),
            );
            Ok(out)
        })
        .await
        .map_err(|e| LaunchError::Game(e.to_string()))??;

        let known: Vec<Known> = others
            .iter()
            .map(|(_, d)| Known {
                sha512: Some(d.sha512.clone()),
                sha1: Some(d.sha1.clone()),
            })
            .collect();
        let matched = if known.is_empty() {
            vec![]
        } else {
            riven_sources::identify(&modrinth, &known)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("cannot look mods up on Modrinth: {e}");
                    vec![None; known.len()]
                })
        };

        let mut disk = HashMap::new();
        for entry in &own.content {
            if let (Some(sha512), Some(path)) = (
                &entry.file.hashes.sha512,
                on_disk(&game_dir, &entry.file.path)?,
            ) {
                disk.insert(sha512.clone(), path);
            }
        }
        let mut taken: HashSet<String> = own.content.iter().map(|e| e.id.clone()).collect();
        let mut content = own.content.clone();
        for ((file, details), found) in others.into_iter().zip(matched) {
            let Ok(path) = PackPath::new(format!("mods/{}", file.name)) else {
                continue;
            };
            disk.insert(details.sha512.clone(), file.path.clone());
            let base = found.as_ref().map_or_else(
                || slug(file.name.trim_end_matches(".jar")),
                |m| m.info.slug.clone(),
            );
            let id = (1..)
                .map(|n| match n {
                    1 => base.clone(),
                    n => format!("{base}-{n}"),
                })
                .find(|id| !taken.contains(id))
                .expect("some suffix is free");
            taken.insert(id.clone());
            let (name, source, url) = match found {
                Some(m) => {
                    let url = m.version.primary_file().and_then(|f| f.url.clone());
                    (m.info.title, m.source, url)
                }
                None => (
                    details.title.unwrap_or_else(|| file.name.clone()),
                    Source::Local { path: path.clone() },
                    None,
                ),
            };
            content.push(Entry {
                id,
                kind: Kind::Mod,
                name,
                source,
                file: EntryFile {
                    path,
                    size: file.size,
                    hashes: Hashes {
                        sha512: Some(details.sha512),
                        sha1: Some(details.sha1),
                    },
                    url,
                },
                side: Side::Both,
                group: None,
                update: UpdatePolicy::Pinned,
                reason: Reason::Explicit,
                requires: vec![],
            });
        }

        let project = Project {
            name: instance.name.clone(),
            id: id.to_owned(),
            version: "0".into(),
            authors: vec![],
            description: None,
            icon: None,
            java: Java {
                major: crate::java::required_major(&instance.minecraft).unwrap_or(21),
                memory: None,
                jvm_args: vec![],
            },
            minecraft: instance.minecraft,
            loader,
            groups: vec![],
            files: Default::default(),
            content,
        };
        let jars = GameJars {
            store: Store::default_location().ok_or(LaunchError::NoDir("data"))?,
            http: riven_sources::client(),
            disk,
        };
        Ok(Self {
            game_dir,
            project,
            mods_locked,
            vanilla,
            own,
            modrinth,
            jars,
        })
    }

    pub fn minecraft(&self) -> &str {
        &self.project.minecraft
    }

    pub fn loader(&self) -> riven_format::LoaderKind {
        self.project.loader.kind
    }

    pub fn modrinth(&self) -> &Modrinth {
        &self.modrinth
    }

    /// Modrinth projects already in the instance, the player's or not.
    pub fn projects(&self) -> HashSet<String> {
        self.project
            .content
            .iter()
            .filter_map(|e| match &e.source {
                Source::Modrinth { project, .. } => Some(project.clone()),
                _ => None,
            })
            .collect()
    }

    fn planner(&self) -> Planner<'_, Modrinth, GameJars> {
        Planner::new(&self.modrinth, &self.jars, &self.project)
    }

    /// Refuses a plan that brings mods where they cannot go.
    fn allowed(&self, plan: Plan) -> Result<Plan, LaunchError> {
        let mods = plan.add.iter().any(|e| e.kind == Kind::Mod);
        match () {
            _ if mods && self.vanilla => Err(LaunchError::NoLoader),
            _ if mods && self.mods_locked => Err(LaunchError::Locked),
            _ => Ok(plan),
        }
    }

    /// Plans a Modrinth project with everything it requires that the instance lacks.
    pub async fn add(&self, project: &str) -> Result<Plan, LaunchError> {
        self.allowed(self.planner().add(project, &AddOptions::default()).await?)
    }

    /// Plans a file of `kind` behind a direct link.
    pub async fn add_url(&self, url: &str, kind: Kind) -> Result<Plan, LaunchError> {
        let name = url::Url::parse(url)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .and_then(|u| {
                let last = u.path_segments()?.next_back()?.to_owned();
                Some(
                    percent_encoding::percent_decode_str(&last)
                        .decode_utf8_lossy()
                        .into_owned(),
                )
            })
            .filter(|n| !n.is_empty())
            .ok_or_else(|| LaunchError::BadLink(url.to_owned()))?;
        let response = self
            .jars
            .http
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| LaunchError::Download(e.to_string()))?;
        let bytes = response
            .bytes()
            .await
            .map_err(|e| LaunchError::Download(e.to_string()))?;
        let file = self.jars.keep(bytes.to_vec(), url)?;
        let source = Source::Url {
            url: url.to_owned(),
        };
        self.add_direct(source, &name, &file, Some(url.to_owned()), kind)
            .await
    }

    /// Plans a file of `kind` from the player's disk; its bytes are kept in the store.
    pub async fn add_file(&self, path: &Path, kind: Kind) -> Result<Plan, LaunchError> {
        let bytes = tokio::fs::read(path).await.map_err(io(path))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file = self.jars.keep(bytes, &path.display().to_string())?;
        let placeholder = Source::Local {
            path: PackPath::new("mods/placeholder.jar").expect("static path is valid"),
        };
        self.add_direct(placeholder, &name, &file, None, kind).await
    }

    async fn add_direct(
        &self,
        source: Source,
        name: &str,
        file: &Downloaded,
        url: Option<String>,
        kind: Kind,
    ) -> Result<Plan, LaunchError> {
        let mut entry = riven_resolve::direct_entry(&self.project, source, name, file, url)?;
        if entry.kind != kind {
            return Err(LaunchError::WrongKind {
                name: name.to_owned(),
                kind,
            });
        }
        if let Source::Local { path } = &mut entry.source {
            *path = entry.file.path.clone();
        }
        self.allowed(self.planner().add_entry(entry).await?)
    }

    /// Newer versions of the player's Modrinth mods.
    pub async fn updates(&self) -> Result<Plan, LaunchError> {
        let ids: Vec<String> = self
            .own
            .content
            .iter()
            .filter(|e| matches!(e.source, Source::Modrinth { .. }))
            .filter(|e| e.update == UpdatePolicy::Follow)
            .filter(|e| !(self.mods_locked && e.kind == Kind::Mod))
            .map(|e| e.id.clone())
            .collect();
        if ids.is_empty() {
            return Ok(Plan::default());
        }
        Ok(self.planner().update(&ids).await?)
    }

    /// Plans removing one of the player's mods and the dependencies nothing else needs.
    pub fn remove(&self, id: &str) -> Result<Plan, LaunchError> {
        let Some(entry) = self.own.content.iter().find(|e| e.id == id) else {
            return Err(LaunchError::NotOwn(id.to_owned()));
        };
        if self.mods_locked && entry.kind == Kind::Mod {
            return Err(LaunchError::Locked);
        }
        Ok(self.planner().remove(id, false)?)
    }

    /// Downloads and places the planned files, then records the player's entries.
    pub async fn apply(&self, plan: &Plan) -> Result<OwnContent, LaunchError> {
        let ours: HashSet<&str> = self.own.content.iter().map(|e| e.id.as_str()).collect();
        if let Some(foreign) = plan
            .remove
            .iter()
            .chain(plan.update.iter().map(|(old, _)| old))
            .find(|e| !ours.contains(e.id.as_str()))
        {
            return Err(LaunchError::NotOwn(foreign.id.clone()));
        }
        for (old, new) in &plan.update {
            let enabled = on_disk(&self.game_dir, &old.file.path)?
                .is_none_or(|p| !p.to_string_lossy().ends_with(DISABLED));
            self.place(new, enabled).await?;
            if old.file.path != new.file.path {
                delete(&self.game_dir, &old.file.path)?;
            }
        }
        for entry in &plan.add {
            self.place(entry, true).await?;
        }
        for entry in &plan.remove {
            delete(&self.game_dir, &entry.file.path)?;
        }

        let mut keep: HashSet<String> = ours.iter().map(|s| (*s).to_owned()).collect();
        keep.extend(plan.add.iter().map(|e| e.id.clone()));
        let mut project = self.project.clone();
        plan.apply(&mut project);
        let mut own = OwnContent {
            content: project
                .content
                .into_iter()
                .filter(|e| keep.contains(&e.id))
                .collect(),
        };
        let ids: HashSet<String> = own.content.iter().map(|e| e.id.clone()).collect();
        for entry in &mut own.content {
            entry.requires.retain(|r| ids.contains(r));
        }
        save(&self.game_dir, &own)?;
        Ok(own)
    }

    async fn place(&self, entry: &Entry, enabled: bool) -> Result<(), LaunchError> {
        let stored = self
            .jars
            .stored(&entry.file)
            .await
            .map_err(LaunchError::Download)?;
        let mut target = install::safe_target(&self.game_dir, &entry.file.path)?;
        if !enabled {
            target = PathBuf::from(format!("{}{DISABLED}", target.display()));
        }
        let parent = target.parent().expect("pack paths have a folder");
        std::fs::create_dir_all(parent).map_err(io(parent))?;
        let temp = parent.join(format!(".{}.part", entry.file.path.file_name()));
        let _ = std::fs::remove_file(&temp);
        if std::fs::hard_link(&stored, &temp).is_err() {
            std::fs::copy(&stored, &temp).map_err(io(&temp))?;
        }
        std::fs::rename(&temp, &target).map_err(io(&target))
    }
}

/// Whether a release file is the same mod as one of the player's: same path, bytes or project.
fn same_mod(entry: &Entry, path: &PackPath, hashes: &Hashes, urls: &[String]) -> bool {
    if &entry.file.path == path {
        return true;
    }
    if let (Some(a), Some(b)) = (&entry.file.hashes.sha512, &hashes.sha512)
        && a.eq_ignore_ascii_case(b)
    {
        return true;
    }
    match &entry.source {
        Source::Modrinth { project, .. } => {
            let marker = format!("cdn.modrinth.com/data/{project}/");
            urls.iter().any(|u| u.contains(&marker))
        }
        _ => false,
    }
}

/// Drops the player's mods a pack release now ships itself, returning their names.
pub fn yield_to_pack(
    game_dir: &Path,
    release: &Release,
    installed: &BTreeMap<PackPath, StateFile>,
) -> Result<Vec<String>, LaunchError> {
    let mut own = load(game_dir)?;
    let shipped: Vec<_> = release
        .files
        .iter()
        .filter(|f| installed.contains_key(&f.path))
        .collect();
    let mut dropped = Vec::new();
    let mut kept = Vec::new();
    for entry in own.content {
        let replaced = shipped
            .iter()
            .find(|f| same_mod(&entry, &f.path, &f.hashes, &f.urls));
        match replaced {
            Some(file) if file.path == entry.file.path => {
                let target = install::safe_target(game_dir, &entry.file.path)?;
                let disabled = PathBuf::from(format!("{}{DISABLED}", target.display()));
                if disabled.is_file() {
                    std::fs::remove_file(&disabled).map_err(io(&disabled))?;
                }
                dropped.push(entry.name);
            }
            Some(_) => {
                delete(game_dir, &entry.file.path)?;
                dropped.push(entry.name);
            }
            None => kept.push(entry),
        }
    }
    if dropped.is_empty() {
        return Ok(dropped);
    }
    let ids: HashSet<String> = kept.iter().map(|e| e.id.clone()).collect();
    for entry in &mut kept {
        entry.requires.retain(|r| ids.contains(r));
    }
    own.content = kept;
    save(game_dir, &own)?;
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use riven_format::{Group, LoaderKind, ReleaseFile};

    use super::*;

    fn own_entry(id: &str, path: &str, source: Source) -> Entry {
        Entry {
            id: id.into(),
            kind: Kind::Mod,
            name: id.into(),
            source,
            file: EntryFile {
                path: PackPath::new(path).unwrap(),
                size: 1,
                hashes: Hashes {
                    sha512: Some(format!("{id:0>128}")),
                    sha1: None,
                },
                url: None,
            },
            side: Side::Both,
            group: None,
            update: UpdatePolicy::Follow,
            reason: Reason::Explicit,
            requires: vec![],
        }
    }

    fn release_file(path: &str, url: &str) -> ReleaseFile {
        ReleaseFile {
            path: PackPath::new(path).unwrap(),
            size: 1,
            hashes: Hashes {
                sha512: Some("f".repeat(128)),
                sha1: None,
            },
            urls: vec![url.into()],
            side: riven_format::Side::Both,
            group: None,
            preserve: false,
        }
    }

    #[test]
    fn pack_wins_over_own_mods_of_the_same_project_or_path() {
        let dir = std::env::temp_dir().join(format!("riven-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mods")).unwrap();
        for name in [
            "sodium-0.6.jar",
            "jei.jar",
            "jei.jar.disabled",
            "mine.jar",
            "pack.jar",
        ] {
            std::fs::write(dir.join("mods").join(name), name).unwrap();
        }
        let sodium = Source::Modrinth {
            project: "AANobbMI".into(),
            version: "old".into(),
        };
        let mut mine = own_entry("mine", "mods/mine.jar", Source::Url { url: "x".into() });
        mine.requires = vec!["sodium".into()];
        save(
            &dir,
            &OwnContent {
                content: vec![
                    own_entry("sodium", "mods/sodium-0.6.jar", sodium),
                    own_entry("jei", "mods/jei.jar", Source::Url { url: "y".into() }),
                    mine,
                ],
            },
        )
        .unwrap();

        let files = vec![
            release_file(
                "mods/sodium-0.7.jar",
                "https://cdn.modrinth.com/data/AANobbMI/versions/new/sodium-0.7.jar",
            ),
            release_file("mods/jei.jar", "../blobs/ab/abc"),
            release_file("mods/pack.jar", "../blobs/cd/cde"),
        ];
        let installed = files
            .iter()
            .map(|f| {
                let record = StateFile {
                    sha512: "f".repeat(128),
                    preserve: false,
                };
                (f.path.clone(), record)
            })
            .collect();
        let release = Release {
            id: "pack".into(),
            name: "Pack".into(),
            version: "2".into(),
            minecraft: "1.21.1".into(),
            loader: riven_format::Loader {
                kind: LoaderKind::NeoForge,
                version: "21.1.77".into(),
            },
            java: Java {
                major: 21,
                memory: None,
                jvm_args: vec![],
            },
            groups: Vec::<Group>::new(),
            files,
            icon: None,
        };

        let mut dropped = yield_to_pack(&dir, &release, &installed).unwrap();
        dropped.sort();
        assert_eq!(dropped, ["jei", "sodium"]);
        assert!(!dir.join("mods/sodium-0.6.jar").exists());
        // Same path: the pack overwrote it, only the player's disabled copy goes.
        assert!(dir.join("mods/jei.jar").exists());
        assert!(!dir.join("mods/jei.jar.disabled").exists());
        let own = load(&dir).unwrap();
        assert_eq!(own.content.len(), 1);
        assert!(own.content[0].requires.is_empty());
        assert!(
            yield_to_pack(&dir, &release, &installed)
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
