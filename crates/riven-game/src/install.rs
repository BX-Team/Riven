use std::collections::{BTreeMap, HashSet};
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::forge::Kind;
use crate::net::Download;
use crate::version::{Features, Library, allowed};
use crate::{
    Dirs, Error, Event, Launch, Prepared, Report, Step, forge, io, maven, net, os, processors,
};

const LIBRARIES: &str = "https://libraries.minecraft.net/";
const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/";
const MAVEN_CENTRAL: &str = "https://repo1.maven.org/maven2/";
const RESOURCES: &str = "https://resources.download.minecraft.net/";

#[derive(Debug, Default)]
pub struct Libraries {
    pub classpath: Vec<PathBuf>,
    pub natives: Vec<Native>,
    pub downloads: Vec<Download>,
}

/// A jar whose native code is unpacked before the game starts.
#[derive(Debug)]
pub struct Native {
    pub jar: PathBuf,
    pub exclude: Vec<String>,
    /// LWJGL 3 jars nest libraries in platform folders; only the files themselves are wanted.
    pub flat: bool,
}

#[derive(Debug, Deserialize)]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: BTreeMap<String, AssetObject>,
    /// 1.6 and 1.7.2 read assets by name from `assets/virtual/<index>`.
    #[serde(default, rename = "virtual")]
    pub is_virtual: bool,
    /// Before 1.6 the game reads them from `<game>/resources`.
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

/// Downloads and unpacks everything the game needs, then runs the loader installer's steps.
pub async fn install(
    http: &reqwest::Client,
    dirs: &Dirs,
    prepared: &Prepared,
    launch: &Launch,
    report: Report<'_>,
) -> Result<(), Error> {
    report(Event::Step(Step::Files));
    let libs = libraries(dirs, prepared);
    let mut downloads = libs.downloads;
    if let Some(client) = prepared
        .version
        .downloads
        .as_ref()
        .and_then(|d| d.client.as_ref())
    {
        downloads.push(
            Download::new(&client.url, prepared.client_jar(dirs))
                .sha1(client.sha1.as_deref())
                .size(client.size),
        );
    }
    if let Some(forge::Installer {
        kind: Kind::Modern(profile),
        ..
    }) = &prepared.installer
    {
        for lib in &profile.libraries {
            downloads.extend(artifact(dirs, lib));
        }
    }
    let index = asset_index(http, dirs, prepared).await?;
    if let Some((_, index)) = &index {
        downloads.extend(index.objects.values().map(|object| {
            let hash = &object.hash;
            let prefix = hash.get(..2).unwrap_or_default();
            Download::new(
                format!("{RESOURCES}{prefix}/{hash}"),
                dirs.assets().join("objects").join(prefix).join(hash),
            )
            .sha1(Some(hash))
            .size(Some(object.size))
        }));
    }
    let (installer, libraries) = (prepared.installer.clone(), dirs.libraries());
    let downloads = crate::blocking(move || {
        if let Some(installer) = &installer {
            forge::unpack(installer, &libraries)?;
        }
        let mut seen = HashSet::new();
        Ok(downloads
            .into_iter()
            .filter(|d| !d.urls.is_empty() && seen.insert(d.path.clone()) && !d.is_done())
            .collect::<Vec<_>>())
    })
    .await?;
    net::fetch_all(http, &downloads, report).await?;

    let (dirs_c, prepared_c, launch_c) = (dirs.clone(), prepared.clone(), launch.clone());
    let natives = libs.natives;
    crate::blocking(move || {
        if let Some((id, index)) = &index {
            place_legacy_assets(&dirs_c, id, index, &launch_c.game_dir)?;
        }
        let target = match natives_subdir(&prepared_c) {
            Some(sub) => launch_c.natives_dir.join(sub),
            None => launch_c.natives_dir.clone(),
        };
        let _ = std::fs::remove_dir_all(&launch_c.natives_dir);
        std::fs::create_dir_all(&target).map_err(io(&target))?;
        for native in &natives {
            extract_native(native, &target)?;
        }
        Ok(())
    })
    .await?;

    if let Some(forge::Installer {
        jar,
        kind: Kind::Modern(profile),
    }) = &prepared.installer
    {
        processors::run(dirs, prepared, jar, profile, &launch.java, report).await?;
    }
    if let Some(missing) = libs.classpath.iter().find(|p| !p.is_file()) {
        return Err(Error::Missing(
            missing.display().to_string(),
            "the loader installer did not produce it".into(),
        ));
    }
    Ok(())
}

impl Prepared {
    pub(crate) fn client_jar(&self, dirs: &Dirs) -> PathBuf {
        let mc = &self.minecraft;
        dirs.versions().join(mc).join(format!("{mc}.jar"))
    }
}

/// Resolves the version's libraries for this system: rules applied, duplicates dropped, the first one winning.
pub fn libraries(dirs: &Dirs, prepared: &Prepared) -> Libraries {
    let x64_java = prepared.needs_x64_java();
    let root = dirs.libraries();
    let mut out = Libraries::default();
    let mut seen = HashSet::new();
    for lib in &prepared.version.libraries {
        if lib.clientreq == Some(false)
            || !allowed(
                lib.rules.as_deref().unwrap_or_default(),
                Features::default(),
            )
        {
            continue;
        }
        let Some(coord) = maven::Coord::parse(&lib.name) else {
            tracing::warn!("skipping library `{}`: not a Maven coordinate", lib.name);
            continue;
        };
        if let Some(classifier) = lib.natives.as_ref().and_then(|n| n.get(os::NAME)) {
            let classifier = classifier.replace("${arch}", os::BITS);
            if seen.insert(format!("{}:{classifier}", coord.key())) {
                let download = native_artifact(dirs, lib, &coord, &classifier);
                out.natives.push(Native {
                    jar: download.path.clone(),
                    exclude: lib.extract.clone().unwrap_or_default().exclude,
                    flat: false,
                });
                out.downloads.push(download);
            }
        }
        let flat = match coord.classifier.and_then(|c| native_fits(c, x64_java)) {
            Some(false) => continue,
            Some(true) => true,
            None => false,
        };
        let has_jar =
            lib.natives.is_none() || lib.downloads.as_ref().is_some_and(|d| d.artifact.is_some());
        if !has_jar || !seen.insert(coord.key()) {
            continue;
        }
        let download = artifact(dirs, lib)
            .unwrap_or_else(|| Download::new(String::new(), root.join(coord.path())));
        if flat {
            out.natives.push(Native {
                jar: download.path.clone(),
                exclude: vec!["META-INF/".into()],
                flat: true,
            });
        }
        out.classpath.push(download.path.clone());
        out.downloads.push(download);
    }
    out
}

/// `Some(true)` for an LWJGL 3 natives classifier of this system, `Some(false)` for another one.
fn native_fits(classifier: &str, x64_java: bool) -> Option<bool> {
    let rest = classifier.strip_prefix("natives-")?;
    let names: &[&str] = match os::NAME {
        "osx" => &["macos", "osx"],
        name => &[name],
    };
    let Some(arch) = names.iter().find_map(|n| rest.strip_prefix(n)) else {
        return Some(false);
    };
    Some(arch.trim_start_matches('-') == os::native_arch(x64_java))
}

/// Where a library comes from, by its `downloads` or by its name in a repository.
fn artifact(dirs: &Dirs, lib: &Library) -> Option<Download> {
    let root = dirs.libraries();
    if let Some(artifact) = lib.downloads.as_ref().and_then(|d| d.artifact.as_ref()) {
        let path = match &artifact.path {
            Some(path) => path.clone(),
            None => maven::path(&lib.name)?,
        };
        let urls = if artifact.url.is_empty() {
            vec![]
        } else {
            vec![artifact.url.clone()]
        };
        return Some(Download {
            urls,
            path: root.join(path),
            checksum: None,
            size: None,
        })
        .map(|d| d.sha1(artifact.sha1.as_deref()).size(artifact.size));
    }
    let path = maven::path(&lib.name)?;
    Some(Download {
        urls: repositories(lib)
            .into_iter()
            .map(|base| format!("{base}{path}"))
            .collect(),
        path: root.join(path),
        checksum: None,
        size: None,
    })
    .map(|d| d.sha1(lib.sha1.as_deref()).size(lib.size))
}

fn native_artifact(dirs: &Dirs, lib: &Library, coord: &maven::Coord, classifier: &str) -> Download {
    let listed = lib
        .downloads
        .as_ref()
        .and_then(|d| d.classifiers.as_ref())
        .and_then(|c| c.get(classifier));
    let path = maven::Coord {
        classifier: Some(classifier),
        ..coord.clone()
    }
    .path();
    match listed {
        Some(a) => Download::new(
            &a.url,
            dirs.libraries().join(a.path.as_ref().unwrap_or(&path)),
        )
        .sha1(a.sha1.as_deref())
        .size(a.size),
        None => Download {
            urls: repositories(lib)
                .into_iter()
                .map(|base| format!("{base}{path}"))
                .collect(),
            path: dirs.libraries().join(&path),
            checksum: None,
            size: None,
        },
    }
}

/// Repositories to try for a library listed by name only; old Forge profiles leave most of them out.
fn repositories(lib: &Library) -> Vec<String> {
    let mut bases = Vec::new();
    match lib.url.as_deref().filter(|u| !u.is_empty()) {
        Some(url) => {
            let mut url = url
                .replace("http://files.minecraftforge.net/maven/", FORGE_MAVEN)
                .replace("http://", "https://");
            if !url.ends_with('/') {
                url.push('/');
            }
            bases.push(url);
        }
        None => bases.extend([LIBRARIES.to_owned(), FORGE_MAVEN.to_owned()]),
    }
    bases.push(MAVEN_CENTRAL.to_owned());
    bases.dedup();
    bases
}

/// The version's asset index, downloaded when missing or changed.
async fn asset_index(
    http: &reqwest::Client,
    dirs: &Dirs,
    prepared: &Prepared,
) -> Result<Option<(String, AssetIndex)>, Error> {
    let Some(index) = &prepared.version.asset_index else {
        return Ok(None);
    };
    let file = Download::new(&index.url, index_path(dirs, &index.id))
        .sha1(index.sha1.as_deref())
        .size(index.size);
    let current = file.path.is_file()
        && index.sha1.as_ref().is_none_or(|sha1| {
            net::sha1_file(&file.path).is_ok_and(|h| h.eq_ignore_ascii_case(sha1))
        });
    if !current && let Err(e) = net::fetch(http, &file, &|_| {}).await {
        if !file.path.is_file() {
            return Err(e);
        }
        tracing::warn!("using the saved asset index {}: {e}", index.id);
    }
    let bytes = std::fs::read(&file.path).map_err(io(&file.path))?;
    Ok(Some((index.id.clone(), crate::json(&index.id, &bytes)?)))
}

pub fn index_path(dirs: &Dirs, id: &str) -> PathBuf {
    dirs.assets().join("indexes").join(format!("{id}.json"))
}

pub fn read_index(dirs: &Dirs, id: &str) -> Option<AssetIndex> {
    let bytes = std::fs::read(index_path(dirs, id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Where old versions read assets from, when not from the object store.
pub fn legacy_assets_dir(
    dirs: &Dirs,
    id: &str,
    index: &AssetIndex,
    game_dir: &Path,
) -> Option<PathBuf> {
    if index.map_to_resources {
        Some(game_dir.join("resources"))
    } else if index.is_virtual {
        Some(dirs.assets().join("virtual").join(id))
    } else {
        None
    }
}

/// Copies objects out under their names for versions that predate the object store.
fn place_legacy_assets(
    dirs: &Dirs,
    id: &str,
    index: &AssetIndex,
    game_dir: &Path,
) -> Result<(), Error> {
    let Some(root) = legacy_assets_dir(dirs, id, index, game_dir) else {
        return Ok(());
    };
    for (name, object) in &index.objects {
        let relative = Path::new(name);
        if !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            continue;
        }
        let target = root.join(relative);
        if std::fs::metadata(&target).is_ok_and(|m| m.len() == object.size) {
            continue;
        }
        let hash = &object.hash;
        let source = dirs
            .assets()
            .join("objects")
            .join(hash.get(..2).unwrap_or_default())
            .join(hash);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::copy(&source, &target).map_err(io(&source))?;
    }
    Ok(())
}

/// The folder under `${natives_directory}` that `java.library.path` points to; 26.1 uses `java`.
pub fn natives_subdir(prepared: &Prepared) -> Option<String> {
    const PREFIX: &str = "-Djava.library.path=${natives_directory}";
    prepared
        .version
        .arguments
        .iter()
        .flat_map(|a| &a.jvm)
        .flat_map(|a| a.values(Features::default()))
        .find_map(|a| a.strip_prefix(PREFIX))
        .map(|sub| sub.trim_start_matches(['/', '\\']).to_owned())
        .filter(|sub| !sub.is_empty())
}

fn is_native_file(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [".dll", ".so", ".dylib", ".jnilib"]
        .iter()
        .any(|ext| name.ends_with(ext))
        || name.rsplit_once(".so.").is_some_and(|(_, v)| {
            !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

fn extract_native(native: &Native, target: &Path) -> Result<(), Error> {
    let jar = &native.jar;
    let file = std::fs::File::open(jar).map_err(io(jar))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|source| Error::Zip {
        path: jar.clone(),
        source,
    })?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|source| Error::Zip {
            path: jar.clone(),
            source,
        })?;
        let name = entry.name().map(|n| n.into_owned()).unwrap_or_default();
        if entry.is_dir() || native.exclude.iter().any(|e| name.starts_with(e.as_str())) {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let out = if native.flat {
            if !is_native_file(&name) {
                continue;
            }
            target.join(relative.file_name().unwrap_or_default())
        } else {
            target.join(relative)
        };
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(io(jar))?;
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&out, bytes).map_err(io(&out))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::VersionJson;

    fn prepared(json: &str) -> Prepared {
        Prepared {
            version: serde_json::from_str::<VersionJson>(json).unwrap(),
            minecraft: "1.0".into(),
            installer: None,
        }
    }

    #[test]
    fn lwjgl3_natives_follow_the_system_and_its_cpu() {
        let here = match os::NAME {
            "osx" => "macos",
            name => name,
        };
        let arch = os::native_arch(false);
        let ours = if arch.is_empty() {
            format!("natives-{here}")
        } else {
            format!("natives-{here}-{arch}")
        };
        assert_eq!(native_fits(&ours, false), Some(true));
        assert_eq!(native_fits("natives-solaris", false), Some(false));
        let other_cpu = if arch == "arm64" {
            format!("natives-{here}")
        } else {
            format!("natives-{here}-arm64")
        };
        assert_eq!(native_fits(&other_cpu, false), Some(false));
        assert_eq!(native_fits("sources", false), None);
    }

    #[test]
    fn duplicate_libraries_keep_the_first_and_old_natives_stay_off_the_classpath() {
        let dirs = Dirs::new("/data");
        let version = format!(
            r#"{{"libraries":[
                {{"name":"org.ow2.asm:asm:9.8","url":"https://maven.fabricmc.net/"}},
                {{"name":"org.ow2.asm:asm:9.6","downloads":{{"artifact":{{"path":"org/ow2/asm/asm/9.6/asm-9.6.jar",
                    "url":"https://libraries.minecraft.net/org/ow2/asm/asm/9.6/asm-9.6.jar","sha1":"aa","size":1}}}}}},
                {{"name":"org.lwjgl.lwjgl:lwjgl-platform:2.9.1","natives":{{"{os}":"natives-{os}"}},
                    "extract":{{"exclude":["META-INF/"]}},
                    "downloads":{{"classifiers":{{"natives-{os}":{{"path":"lwjgl-platform-natives.jar",
                    "url":"https://libraries.minecraft.net/lwjgl-platform-natives.jar"}}}}}}}},
                {{"name":"com.mojang:server-only:1.0","clientreq":false}},
                {{"name":"net.minecraftforge:forge:1.20.1-47.4.0:client","downloads":{{"artifact":{{
                    "path":"net/minecraftforge/forge/1.20.1-47.4.0/forge-1.20.1-47.4.0-client.jar","url":""}}}}}}
            ]}}"#,
            os = os::NAME
        );
        let libs = libraries(&dirs, &prepared(&version));
        let root = dirs.libraries();
        assert_eq!(
            libs.classpath,
            [
                root.join("org/ow2/asm/asm/9.8/asm-9.8.jar"),
                root.join("net/minecraftforge/forge/1.20.1-47.4.0/forge-1.20.1-47.4.0-client.jar"),
            ]
        );
        assert_eq!(libs.natives.len(), 1);
        assert_eq!(libs.natives[0].jar, root.join("lwjgl-platform-natives.jar"));
        let asm = &libs.downloads[0];
        assert_eq!(
            asm.urls,
            [
                "https://maven.fabricmc.net/org/ow2/asm/asm/9.8/asm-9.8.jar",
                "https://repo1.maven.org/maven2/org/ow2/asm/asm/9.8/asm-9.8.jar"
            ]
        );
        let patched = libs.downloads.last().unwrap();
        assert!(
            patched.urls.is_empty(),
            "processor outputs are not downloaded"
        );
    }

    #[test]
    fn natives_follow_the_library_path_subfolder() {
        let modern = prepared(
            r#"{"arguments":{"jvm":["-Djava.library.path=${natives_directory}/java","-cp","${classpath}"]}}"#,
        );
        assert_eq!(natives_subdir(&modern).as_deref(), Some("java"));
        let old = prepared(r#"{"arguments":{"jvm":["-Djava.library.path=${natives_directory}"]}}"#);
        assert_eq!(natives_subdir(&old), None);
        assert!(is_native_file("liblwjgl.so.3.3.3"));
        assert!(!is_native_file("liblwjgl.so.sha1"));
    }
}
