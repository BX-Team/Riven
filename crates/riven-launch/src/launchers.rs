use std::path::{Component, Path, PathBuf};

use riven_format::{Loader, LoaderKind};
use serde::Deserialize;

use crate::instances::Instances;
use crate::{LaunchError, io};

/// A launcher whose instances Riven can copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Launcher {
    Prism,
    MultiMc,
    CurseForge,
    Modrinth,
}

impl Launcher {
    pub const ALL: [Self; 4] = [Self::Prism, Self::MultiMc, Self::CurseForge, Self::Modrinth];

    pub fn name(self) -> &'static str {
        match self {
            Self::Prism => "Prism Launcher",
            Self::MultiMc => "MultiMC",
            Self::CurseForge => "CurseForge",
            Self::Modrinth => "Modrinth App",
        }
    }

    /// Where the launcher keeps its data when the player did not move it.
    fn usual_places(self) -> Vec<PathBuf> {
        let data = dirs::data_dir();
        let home = dirs::home_dir();
        let mut places: Vec<PathBuf> = match self {
            Self::Prism => data.iter().map(|d| d.join("PrismLauncher")).collect(),
            Self::MultiMc => data
                .iter()
                .flat_map(|d| [d.join("multimc"), d.join("MultiMC")])
                .chain(home.iter().map(|h| h.join("MultiMC")))
                .collect(),
            Self::CurseForge => home
                .iter()
                .map(|h| h.join("curseforge").join("minecraft"))
                .chain(dirs::document_dir().map(|d| d.join("curseforge").join("minecraft")))
                .collect(),
            Self::Modrinth => data
                .iter()
                .flat_map(|d| [d.join("ModrinthApp"), d.join("com.modrinth.theseus")])
                .collect(),
        };
        if cfg!(target_os = "linux")
            && self == Self::Prism
            && let Some(home) = &home
        {
            places.push(home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher"));
        }
        if cfg!(target_os = "macos")
            && self == Self::MultiMc
            && let Some(home) = &home
        {
            places.push(PathBuf::from("/Applications/MultiMC.app/Data"));
            places.push(home.join("Applications/MultiMC.app/Data"));
        }
        places
    }

    /// Whether `dir` is this launcher's data folder.
    fn owns(self, dir: &Path) -> bool {
        match self {
            Self::Prism => dir.join("prismlauncher.cfg").is_file(),
            Self::MultiMc => dir.join("multimc.cfg").is_file(),
            Self::CurseForge => dir.join("Instances").is_dir(),
            Self::Modrinth => dir.join("app.db").is_file(),
        }
    }
}

/// An instance of another launcher, read but not yet copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreign {
    pub launcher: Launcher,
    pub name: String,
    pub minecraft: String,
    pub loader: Option<Loader>,
    /// The folder the game runs in; its contents become the new instance's game folder.
    pub game_dir: PathBuf,
    pub icon: Option<PathBuf>,
    /// Launcher files inside `game_dir` that are not the game's.
    skip: &'static [&'static str],
}

fn unreadable(path: &Path, why: impl std::fmt::Display) -> LaunchError {
    LaunchError::Foreign(format!("{}: {why}", path.display()))
}

/// The launcher folders found in their usual places.
pub fn installed() -> Vec<(Launcher, PathBuf)> {
    let mut found: Vec<(Launcher, PathBuf)> = Vec::new();
    for launcher in Launcher::ALL {
        for place in launcher.usual_places() {
            let place = dunce::canonicalize(&place).unwrap_or(place);
            if launcher.owns(&place) && !found.iter().any(|(_, p)| *p == place) {
                found.push((launcher, place));
            }
        }
    }
    found
}

/// The launcher a folder the player picked belongs to.
pub fn identify(dir: &Path) -> Option<Launcher> {
    [
        Launcher::Modrinth,
        Launcher::Prism,
        Launcher::MultiMc,
        Launcher::CurseForge,
    ]
    .into_iter()
    .find(|l| l.owns(dir))
}

/// The instances of a launcher at `root`, sorted by name; ones that cannot be read are skipped.
pub fn scan(launcher: Launcher, root: &Path) -> Result<Vec<Foreign>, LaunchError> {
    let mut found = match launcher {
        Launcher::Prism => scan_mmc(launcher, root, "prismlauncher.cfg")?,
        Launcher::MultiMc => scan_mmc(launcher, root, "multimc.cfg")?,
        Launcher::CurseForge => scan_curseforge(root)?,
        Launcher::Modrinth => scan_modrinth(root)?,
    };
    found.sort_by_key(|f| f.name.to_lowercase());
    Ok(found)
}

/// Subfolders of `dir`; a missing folder has none.
fn subdirs(dir: &Path) -> Result<Vec<PathBuf>, LaunchError> {
    match std::fs::read_dir(dir) {
        Ok(entries) => Ok(entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(io(dir)(e)),
    }
}

fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A value from a Qt INI file, which quotes values with commas or edge spaces.
fn ini(text: &str, key: &str) -> Option<String> {
    let raw = text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| v.trim())
    })?;
    let value = match raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        Some(inner) => inner.replace("\\\"", "\"").replace("\\\\", "\\"),
        None => raw.to_owned(),
    };
    (!value.is_empty()).then_some(value)
}

/// A loader as another launcher names it; versions may carry the game version around them.
fn loader(name: &str, version: Option<&str>, minecraft: &str) -> Option<Loader> {
    let kind = match name.to_ascii_lowercase().as_str() {
        "fabric" => LoaderKind::Fabric,
        "quilt" => LoaderKind::Quilt,
        "forge" => LoaderKind::Forge,
        "neoforge" => LoaderKind::NeoForge,
        _ => return None,
    };
    let version = version?.trim();
    let version = version
        .strip_prefix(&format!("{minecraft}-"))
        .unwrap_or(version);
    let version = version
        .strip_suffix(&format!("-{minecraft}"))
        .unwrap_or(version);
    (!version.is_empty()).then(|| Loader {
        kind,
        version: version.to_owned(),
    })
}

/// A launcher setting holding a folder, relative to the launcher's own.
fn setting_dir(root: &Path, cfg: Option<&str>, key: &str, default: &str) -> PathBuf {
    let value = cfg.and_then(|c| ini(c, key));
    root.join(value.as_deref().unwrap_or(default))
}

#[derive(Deserialize)]
struct MmcPack {
    components: Vec<MmcComponent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MmcComponent {
    uid: String,
    version: Option<String>,
    cached_version: Option<String>,
}

fn scan_mmc(launcher: Launcher, root: &Path, cfg_name: &str) -> Result<Vec<Foreign>, LaunchError> {
    let cfg = std::fs::read_to_string(root.join(cfg_name)).ok();
    let instances = setting_dir(root, cfg.as_deref(), "InstanceDir", "instances");
    let icons = setting_dir(root, cfg.as_deref(), "IconsDir", "icons");
    Ok(subdirs(&instances)?
        .into_iter()
        .filter_map(|dir| {
            let found = read_mmc(launcher, &dir, &icons);
            if found.is_none() && dir.join("mmc-pack.json").is_file() {
                tracing::warn!("skipping {}: unreadable instance", dir.display());
            }
            found
        })
        .collect())
}

fn read_mmc(launcher: Launcher, dir: &Path, icons: &Path) -> Option<Foreign> {
    let cfg = std::fs::read_to_string(dir.join("instance.cfg")).ok()?;
    let pack: MmcPack =
        serde_json::from_str(&std::fs::read_to_string(dir.join("mmc-pack.json")).ok()?).ok()?;
    let version = |uid: &str| {
        let c = pack.components.iter().find(|c| c.uid == uid)?;
        c.version.clone().or_else(|| c.cached_version.clone())
    };
    let minecraft = version("net.minecraft")?;
    let loader = [
        ("net.neoforged", "neoforge"),
        ("net.minecraftforge", "forge"),
        ("net.fabricmc.fabric-loader", "fabric"),
        ("org.quiltmc.quilt-loader", "quilt"),
    ]
    .into_iter()
    .find_map(|(uid, name)| loader(name, version(uid).as_deref(), &minecraft));
    let game_dir = ["minecraft", ".minecraft"]
        .into_iter()
        .map(|n| dir.join(n))
        .find(|d| d.is_dir())
        .unwrap_or_else(|| dir.join(".minecraft"));
    let icon = ini(&cfg, "iconKey")
        .and_then(|key| find_icon(dir, &key).or_else(|| find_icon(icons, &key)));
    Some(Foreign {
        launcher,
        name: ini(&cfg, "name").unwrap_or_else(|| folder_name(dir)),
        minecraft,
        loader,
        game_dir,
        icon,
        skip: &[],
    })
}

/// A custom icon `<key>.<ext>` in `dir`; built-in icons have no file.
fn find_icon(icons: &Path, key: &str) -> Option<PathBuf> {
    std::fs::read_dir(icons)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file() && p.file_stem().is_some_and(|s| s == key))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CurseForgeInstance {
    name: Option<String>,
    game_version: String,
    base_mod_loader: Option<CurseForgeLoader>,
    profile_image_path: Option<PathBuf>,
}

#[derive(Deserialize)]
struct CurseForgeLoader {
    /// `forge-47.3.0`, `neoforge-21.1.77`, `fabric-0.16.5-1.21.1`.
    name: String,
}

const CURSEFORGE_FILE: &str = "minecraftinstance.json";

fn scan_curseforge(root: &Path) -> Result<Vec<Foreign>, LaunchError> {
    Ok(subdirs(&root.join("Instances"))?
        .into_iter()
        .filter_map(|dir| {
            let text = std::fs::read_to_string(dir.join(CURSEFORGE_FILE)).ok()?;
            let parsed: CurseForgeInstance =
                match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        tracing::warn!("skipping {}: {e}", dir.display());
                        return None;
                    }
                };
            let minecraft = parsed.game_version;
            let loader = parsed.base_mod_loader.and_then(|l| {
                let (kind, version) = l.name.split_once('-')?;
                loader(kind, Some(version), &minecraft)
            });
            Some(Foreign {
                launcher: Launcher::CurseForge,
                name: parsed.name.unwrap_or_else(|| folder_name(&dir)),
                minecraft,
                loader,
                icon: parsed.profile_image_path.filter(|p| p.is_file()),
                game_dir: dir,
                skip: &[CURSEFORGE_FILE],
            })
        })
        .collect())
}

/// Instances from the Modrinth App's database: `instances` since mid-2026, `profiles` before.
fn scan_modrinth(root: &Path) -> Result<Vec<Foreign>, LaunchError> {
    let db_path = root.join("app.db");
    let fail = |e: rusqlite::Error| unreadable(&db_path, e);
    // Read-write without create: a read-only open fails on a WAL database whose `-shm` is gone.
    let db = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(fail)?;
    db.pragma_update(None, "query_only", true).map_err(fail)?;
    let has_table = |name: &str| {
        db.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |_| Ok(()),
        )
        .is_ok()
    };
    let query = if has_table("instances") {
        "SELECT i.path, i.name, i.icon_path, s.game_version, s.loader, s.loader_version
         FROM instances i JOIN instance_content_sets s ON s.id = i.applied_content_set_id"
    } else if has_table("profiles") {
        "SELECT path, name, icon_path, game_version, mod_loader, mod_loader_version FROM profiles"
    } else {
        return Err(unreadable(&db_path, "no instances table"));
    };
    let custom: Option<String> = db
        .query_row("SELECT custom_dir FROM settings LIMIT 1", [], |r| r.get(0))
        .ok()
        .flatten();
    let profiles = custom
        .map_or_else(|| root.to_owned(), PathBuf::from)
        .join("profiles");
    let mut statement = db.prepare(query).map_err(fail)?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(fail)?;
    let mut found = Vec::new();
    for row in rows {
        let (path, name, icon, minecraft, kind, version) = row.map_err(fail)?;
        let relative = Path::new(&path);
        if !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            tracing::warn!("skipping Modrinth App instance with path `{path}`");
            continue;
        }
        found.push(Foreign {
            launcher: Launcher::Modrinth,
            name,
            loader: loader(&kind, version.as_deref(), &minecraft),
            minecraft,
            game_dir: profiles.join(relative),
            icon: icon.map(PathBuf::from).filter(|p| p.is_file()),
            skip: &[],
        });
    }
    Ok(found)
}

/// Every file under `dir` relative to it; links are left out, as they may point anywhere, even back up.
fn files(dir: &Path, skip: &[&str]) -> Result<Vec<PathBuf>, LaunchError> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let here = dir.join(&rel);
        let entries = match std::fs::read_dir(&here) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && rel.as_os_str().is_empty() => {
                return Ok(out);
            }
            Err(e) => return Err(io(&here)(e)),
        };
        for entry in entries {
            let entry = entry.map_err(io(&here))?;
            let name = entry.file_name();
            if rel.as_os_str().is_empty() && skip.iter().any(|s| name == **s) {
                continue;
            }
            let kind = entry.file_type().map_err(io(&entry.path()))?;
            if kind.is_dir() {
                stack.push(rel.join(&name));
            } else if kind.is_file() {
                out.push(rel.join(&name));
            }
        }
    }
    Ok(out)
}

/// Copies another launcher's instance into a new one named `name` and returns its id; the original stays.
pub fn import(
    store: &Instances,
    foreign: &Foreign,
    name: &str,
    mut progress: impl FnMut(usize, usize),
) -> Result<String, LaunchError> {
    let list = files(&foreign.game_dir, foreign.skip)?;
    let id = store.create(name, &foreign.minecraft, foreign.loader.clone())?;
    let game = store.game_dir(&id);
    let copied = (|| {
        for (done, rel) in list.iter().enumerate() {
            progress(done, list.len());
            let (from, to) = (foreign.game_dir.join(rel), game.join(rel));
            let parent = to.parent().expect("joined paths have a parent");
            std::fs::create_dir_all(parent).map_err(io(parent))?;
            std::fs::copy(&from, &to).map_err(io(&from))?;
        }
        Ok(())
    })();
    if let Err(e) = copied {
        let _ = store.delete(&id);
        return Err(e);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("riven-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn loader_versions_lose_the_game_version() {
        let forge = loader("forge", Some("1.20.1-47.3.0"), "1.20.1").unwrap();
        assert_eq!(
            (forge.kind, forge.version.as_str()),
            (LoaderKind::Forge, "47.3.0")
        );
        let fabric = loader("fabric", Some("0.16.5-1.21.1"), "1.21.1").unwrap();
        assert_eq!(fabric.version, "0.16.5");
        assert_eq!(loader("vanilla", Some("1.0"), "1.21.1"), None);
    }

    #[test]
    fn prism_instances_follow_the_moved_instance_folder() {
        let root = temp("prism");
        write(
            &root.join("prismlauncher.cfg"),
            "[General]\nInstanceDir=\"My Instances\"\n",
        );
        let dir = root.join("My Instances/pack");
        write(
            &dir.join("instance.cfg"),
            "[General]\nname=\"Create, Above\"\niconKey=custom\n",
        );
        write(
            &dir.join("mmc-pack.json"),
            r#"{"components":[{"uid":"net.minecraft","version":"1.20.1"},
                {"uid":"net.minecraftforge","version":"47.3.0"}]}"#,
        );
        write(&dir.join("minecraft/mods/a.jar"), "x");
        write(&root.join("icons/custom.png"), "png");
        assert_eq!(identify(&root), Some(Launcher::Prism));
        let found = scan(Launcher::Prism, &root).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Create, Above");
        assert_eq!(found[0].game_dir, dir.join("minecraft"));
        assert_eq!(found[0].icon, Some(root.join("icons/custom.png")));

        let store = Instances::new(root.join("riven"));
        let id = import(&store, &found[0], "Copy", |_, _| {}).unwrap();
        assert!(store.game_dir(&id).join("mods/a.jar").is_file());
        assert_eq!(store.load(&id).unwrap().loader, found[0].loader);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn modrinth_reads_both_database_layouts() {
        for (table, schema) in [
            (
                "profiles",
                "CREATE TABLE profiles (path TEXT, name TEXT, icon_path TEXT, game_version TEXT,
                    mod_loader TEXT, mod_loader_version TEXT);
                 INSERT INTO profiles VALUES ('pack', 'Pack', NULL, '1.21.1', 'fabric', '0.16.5'),
                    ('../evil', 'Evil', NULL, '1.21.1', 'vanilla', NULL);",
            ),
            (
                "instances",
                "CREATE TABLE instances (id TEXT, path TEXT, name TEXT, icon_path TEXT,
                    applied_content_set_id TEXT);
                 CREATE TABLE instance_content_sets (id TEXT, game_version TEXT, loader TEXT,
                    loader_version TEXT);
                 CREATE TABLE settings (custom_dir TEXT);
                 INSERT INTO settings VALUES (NULL);
                 INSERT INTO instances VALUES ('a', 'pack', 'Pack', NULL, 's');
                 INSERT INTO instance_content_sets VALUES ('s', '1.21.1', 'fabric', '0.16.5');",
            ),
        ] {
            let root = temp(&format!("modrinth-{table}"));
            let db = rusqlite::Connection::open(root.join("app.db")).unwrap();
            db.execute_batch(schema).unwrap();
            drop(db);
            let found = scan(Launcher::Modrinth, &root).unwrap();
            assert_eq!(found.len(), 1, "{table}");
            assert_eq!(found[0].game_dir, root.join("profiles/pack"));
            assert_eq!(found[0].loader.as_ref().unwrap().version, "0.16.5");
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
