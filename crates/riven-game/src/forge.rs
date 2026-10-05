use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::version::{Library, VersionJson};
use crate::{Dirs, Error, GameVersion, Loader, maven, net};

const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/";
const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/";
const FORGE_BUILDS: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json";

/// A Forge or NeoForge installer and what it asks to set up besides downloads.
#[derive(Debug, Clone)]
pub struct Installer {
    pub jar: PathBuf,
    pub kind: Kind,
}

#[derive(Debug, Clone)]
pub enum Kind {
    /// 1.13 and later (and late 1.12.2 builds): libraries bundled in the installer plus processors.
    Modern(Box<InstallProfile>),
    /// 1.6 to 1.12.2: one universal jar inside the installer.
    Legacy { universal: String, file: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstallProfile {
    pub json: Option<String>,
    #[serde(default)]
    pub data: BTreeMap<String, Sided>,
    #[serde(default)]
    pub processors: Vec<Processor>,
    #[serde(default)]
    pub libraries: Vec<Library>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sided {
    pub client: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Processor {
    pub jar: String,
    #[serde(default)]
    pub classpath: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub sides: Option<Vec<String>>,
    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct LegacyProfile {
    install: LegacyInstall,
    #[serde(rename = "versionInfo")]
    version_info: VersionJson,
}

#[derive(Deserialize)]
struct LegacyInstall {
    path: String,
    #[serde(rename = "filePath")]
    file_path: String,
}

/// Fetches the installer once, then reads the version it describes from it.
pub async fn prepare(
    http: &reqwest::Client,
    dirs: &Dirs,
    game: &GameVersion,
) -> Result<(VersionJson, Installer), Error> {
    let jar = installer(http, dirs, game).await?;
    let minecraft = game.minecraft.clone();
    let path = jar.clone();
    let (version, kind) = crate::blocking(move || read(&path, &minecraft)).await?;
    Ok((version, Installer { jar, kind }))
}

fn coordinate(game: &GameVersion, artifact_version: &str) -> String {
    if game.loader == Loader::NeoForge && game.minecraft == "1.20.1" {
        format!("net.neoforged:forge:{artifact_version}:installer")
    } else if game.loader == Loader::NeoForge {
        format!("net.neoforged:neoforge:{artifact_version}:installer")
    } else {
        format!("net.minecraftforge:forge:{artifact_version}:installer")
    }
}

async fn installer(
    http: &reqwest::Client,
    dirs: &Dirs,
    game: &GameVersion,
) -> Result<PathBuf, Error> {
    let (mc, version) = (&game.minecraft, &game.loader_version);
    let candidates = match game.loader {
        Loader::NeoForge if mc == "1.20.1" => vec![format!("{mc}-{version}")],
        Loader::NeoForge => vec![version.clone()],
        _ => vec![format!("{mc}-{version}"), format!("{mc}-{version}-{mc}")],
    };
    for candidate in &candidates {
        let path = dirs
            .libraries()
            .join(maven::path(&coordinate(game, candidate)).unwrap_or_default());
        if path.is_file() {
            return Ok(path);
        }
    }
    let (repo, name) = match game.loader {
        Loader::NeoForge => (NEOFORGE_MAVEN, "NeoForge"),
        _ => (FORGE_MAVEN, "Forge"),
    };
    let missing = || Error::NoLoader(format!("{name} {version} for Minecraft {mc}"));
    let artifact_version = match game.loader {
        Loader::NeoForge => candidates[0].clone(),
        _ => forge_build(http, mc, version).await?.ok_or_else(missing)?,
    };
    let relative = maven::path(&coordinate(game, &artifact_version)).ok_or_else(missing)?;
    let file = net::Download::new(
        format!("{repo}{relative}"),
        dirs.libraries().join(&relative),
    );
    match net::fetch(http, &file, &|_| {}).await {
        Err(Error::Status { status: 404, .. }) => Err(missing()),
        other => other.map(|()| file.path),
    }
}

/// The Maven version of a Forge build: old ones carry the game version twice, `1.7.10-10.13.4.1614-1.7.10`.
async fn forge_build(
    http: &reqwest::Client,
    minecraft: &str,
    version: &str,
) -> Result<Option<String>, Error> {
    let builds: BTreeMap<String, Vec<String>> = net::get_json(http, FORGE_BUILDS).await?;
    let exact = format!("{minecraft}-{version}");
    let suffixed = format!("{exact}-");
    Ok(builds
        .get(minecraft)
        .into_iter()
        .flatten()
        .find(|b| **b == exact || b.starts_with(&suffixed))
        .cloned())
}

fn open(jar: &Path) -> Result<zip::ZipArchive<std::fs::File>, Error> {
    let file = std::fs::File::open(jar).map_err(crate::io(jar))?;
    zip::ZipArchive::new(file).map_err(|source| Error::Zip {
        path: jar.to_owned(),
        source,
    })
}

pub fn entry(
    archive: &mut zip::ZipArchive<std::fs::File>,
    jar: &Path,
    name: &str,
) -> Result<Vec<u8>, Error> {
    let mut file = archive.by_name(name).map_err(|source| Error::Zip {
        path: jar.join(name),
        source,
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(crate::io(jar))?;
    Ok(bytes)
}

fn read(jar: &Path, minecraft: &str) -> Result<(VersionJson, Kind), Error> {
    let mut archive = open(jar)?;
    let profile = entry(&mut archive, jar, "install_profile.json")?;
    let what = format!("{}: install_profile.json", jar.display());
    let raw: serde_json::Value = crate::json(&what, &profile)?;
    if raw.get("versionInfo").is_some() {
        let legacy: LegacyProfile = crate::json(&what, &profile)?;
        let mut version = legacy.version_info;
        version
            .inherits_from
            .get_or_insert_with(|| minecraft.to_owned());
        let universal = maven::path(&legacy.install.path).ok_or_else(|| {
            Error::Unsupported(format!("unknown Forge library {}", legacy.install.path))
        })?;
        return Ok((
            version,
            Kind::Legacy {
                universal,
                file: legacy.install.file_path,
            },
        ));
    }
    let profile: InstallProfile = crate::json(&what, &profile)?;
    let name = profile.json.as_deref().unwrap_or("/version.json");
    let bytes = entry(&mut archive, jar, name.trim_start_matches('/'))?;
    let version = crate::json(&format!("{}: {name}", jar.display()), &bytes)?;
    Ok((version, Kind::Modern(Box::new(profile))))
}

/// Unpacks what the installer carries itself: the universal jar, or the `maven/` folder.
pub fn unpack(installer: &Installer, libraries: &Path) -> Result<(), Error> {
    let jar = &installer.jar;
    let mut archive = open(jar)?;
    match &installer.kind {
        Kind::Legacy { universal, file } => {
            let target = libraries.join(universal);
            if !target.is_file() {
                let bytes = entry(&mut archive, jar, file)?;
                net::save(&target, &bytes)?;
            }
        }
        Kind::Modern(_) => {
            for i in 0..archive.len() {
                let mut file = archive.by_index(i).map_err(|source| Error::Zip {
                    path: jar.clone(),
                    source,
                })?;
                let Some(relative) = file
                    .enclosed_name()
                    .and_then(|p| p.strip_prefix("maven").ok().map(Path::to_owned))
                else {
                    continue;
                };
                if file.is_dir() || relative.as_os_str().is_empty() {
                    continue;
                }
                let target = libraries.join(relative);
                if std::fs::metadata(&target).is_ok_and(|m| m.len() == file.size()) {
                    continue;
                }
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).map_err(crate::io(jar))?;
                net::save(&target, &bytes)?;
            }
        }
    }
    Ok(())
}
