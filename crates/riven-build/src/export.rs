use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use riven_format::{Entry, InstallSide, LoaderKind, Project, Side};
use serde_json::json;
use zip::write::SimpleFileOptions;

use crate::import::Scope;
use crate::release::{Error, OverrideFile, collect_overrides};

/// Download hosts the `.mrpack` format allows; anything else must be embedded.
const MRPACK_HOSTS: &[&str] = &[
    "cdn.modrinth.com",
    "github.com",
    "raw.githubusercontent.com",
    "gitlab.com",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mrpack,
    Prism,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Mrpack => "mrpack",
            Format::Prism => "zip",
        }
    }
}

/// What an export did with the pack's entries.
#[derive(Debug, Default)]
pub struct Exported {
    pub linked: usize,
    pub embedded: usize,
    /// Entries left out, with the reason.
    pub skipped: Vec<(String, String)>,
}

fn group_default(project: &Project, entry: &Entry) -> bool {
    entry.group.as_ref().is_none_or(|g| {
        project
            .groups
            .iter()
            .find(|x| &x.id == g)
            .is_some_and(|x| x.default)
    })
}

fn mrpack_link(entry: &Entry) -> Option<&str> {
    let url = entry.file.url.as_deref()?;
    let host = url::Url::parse(url).ok()?.host_str()?.to_owned();
    let hashes = &entry.file.hashes;
    (MRPACK_HOSTS.contains(&host.as_str()) && hashes.sha1.is_some() && hashes.sha512.is_some())
        .then_some(url)
}

/// Whether `format` must embed `entry`'s bytes rather than link it.
pub fn needs_bytes(format: Format, entry: &Entry) -> bool {
    match format {
        Format::Mrpack => mrpack_link(entry).is_none(),
        Format::Prism => true,
    }
}

/// `.mrpack` carries both sides; Prism packs are client packs unless told otherwise.
fn effective_side(format: Format, side: Option<InstallSide>) -> Option<InstallSide> {
    match format {
        Format::Mrpack => side,
        Format::Prism => Some(side.unwrap_or(InstallSide::Client)),
    }
}

/// Entries an export of `format` for `side` includes.
pub fn selected(format: Format, project: &Project, side: Option<InstallSide>) -> Vec<&Entry> {
    let side = effective_side(format, side);
    project
        .content
        .iter()
        .filter(|e| side.is_none_or(|s| e.side.includes(s)))
        .filter(|e| format != Format::Prism || group_default(project, e))
        .collect()
}

/// Jars and pack zips are already compressed; deflating them again only costs time.
fn options(name: &str) -> SimpleFileOptions {
    let method = if name.ends_with(".jar") || name.ends_with(".zip") {
        zip::CompressionMethod::Stored
    } else {
        zip::CompressionMethod::Deflated
    };
    SimpleFileOptions::default()
        .compression_method(method)
        .last_modified_time(zip::DateTime::default())
}

struct Archive {
    zip: zip::ZipWriter<BufWriter<File>>,
    path: std::path::PathBuf,
}

impl Archive {
    fn create(path: &Path) -> Result<Self, Error> {
        let file = File::create(path).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self {
            zip: zip::ZipWriter::new(BufWriter::new(file)),
            path: path.to_owned(),
        })
    }

    fn add(&mut self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        let io = |e: std::io::Error| Error::Io {
            path: self.path.clone(),
            source: e,
        };
        self.zip
            .start_file(name, options(name))
            .map_err(|e| io(std::io::Error::other(e)))?;
        self.zip.write_all(bytes).map_err(io)
    }

    fn finish(self) -> Result<(), Error> {
        let path = self.path;
        let mut writer = self.zip.finish().map_err(|e| Error::Io {
            path: path.clone(),
            source: std::io::Error::other(e),
        })?;
        writer.flush().map_err(|source| Error::Io { path, source })
    }
}

fn read(path: &Path) -> Result<Vec<u8>, Error> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}

fn wanted_override(file: &OverrideFile, side: Option<InstallSide>) -> bool {
    match (file.scope, side) {
        (Scope::Common, _) | (_, None) => true,
        (Scope::Client, Some(side)) => side == InstallSide::Client,
        (Scope::Server, Some(side)) => side == InstallSide::Server,
    }
}

/// Writes `project` as `format` to `out`; `bytes` supplies files the format must embed.
pub fn export(
    format: Format,
    project: &Project,
    repo: &Path,
    side: Option<InstallSide>,
    bytes: &dyn Fn(&Entry) -> Result<Vec<u8>, String>,
    out: &Path,
) -> Result<Exported, Error> {
    let mut archive = Archive::create(out)?;
    let side = effective_side(format, side);
    let mut report = Exported::default();
    let entries = selected(format, project, side);
    let overrides: Vec<OverrideFile> = collect_overrides(repo, project)?
        .into_iter()
        .filter(|f| wanted_override(f, side))
        .collect();
    let embed = |archive: &mut Archive,
                 report: &mut Exported,
                 entry: &Entry,
                 prefix: &str|
     -> Result<(), Error> {
        match bytes(entry) {
            Ok(data) => {
                archive.add(&format!("{prefix}{}", entry.file.path), &data)?;
                report.embedded += 1;
            }
            Err(e) => report.skipped.push((entry.id.clone(), e)),
        }
        Ok(())
    };

    match format {
        Format::Mrpack => {
            let mut files = Vec::new();
            for entry in &entries {
                match mrpack_link(entry) {
                    Some(url) => {
                        let env = |s: InstallSide| match (
                            entry.side.includes(s),
                            group_default(project, entry),
                        ) {
                            (false, _) => "unsupported",
                            (true, true) => "required",
                            (true, false) => "optional",
                        };
                        files.push(json!({
                            "path": entry.file.path,
                            "hashes": { "sha1": entry.file.hashes.sha1, "sha512": entry.file.hashes.sha512 },
                            "env": { "client": env(InstallSide::Client), "server": env(InstallSide::Server) },
                            "downloads": [url],
                            "fileSize": entry.file.size,
                        }));
                        report.linked += 1;
                    }
                    None => {
                        let prefix = match entry.side {
                            Side::Both => "overrides/",
                            Side::Client => "client-overrides/",
                            Side::Server => "server-overrides/",
                        };
                        embed(&mut archive, &mut report, entry, prefix)?;
                    }
                }
            }
            let loader = match project.loader.kind {
                LoaderKind::NeoForge => "neoforge",
                LoaderKind::Forge => "forge",
                LoaderKind::Fabric => "fabric-loader",
                LoaderKind::Quilt => "quilt-loader",
            };
            let index = json!({
                "formatVersion": 1,
                "game": "minecraft",
                "versionId": project.version,
                "name": project.name,
                "summary": project.description,
                "files": files,
                "dependencies": { "minecraft": project.minecraft, loader: project.loader.version },
            });
            archive.add(
                "modrinth.index.json",
                serde_json::to_string_pretty(&index)
                    .expect("json values serialize")
                    .as_bytes(),
            )?;
            for file in &overrides {
                let prefix = match file.scope {
                    Scope::Common => "overrides/",
                    Scope::Client => "client-overrides/",
                    Scope::Server => "server-overrides/",
                };
                archive.add(&format!("{prefix}{}", file.path), &read(&file.source)?)?;
            }
        }
        Format::Prism => {
            for entry in &entries {
                embed(&mut archive, &mut report, entry, ".minecraft/")?;
            }
            for file in &overrides {
                archive.add(&format!(".minecraft/{}", file.path), &read(&file.source)?)?;
            }
            archive.add("mmc-pack.json", prism_components(project).as_bytes())?;
            archive.add("instance.cfg", prism_instance(project).as_bytes())?;
        }
    }
    archive.finish()?;
    Ok(report)
}

fn prism_components(project: &Project) -> String {
    let mc = &project.minecraft;
    let loader = &project.loader.version;
    let mut components = vec![json!({ "uid": "net.minecraft", "version": mc, "important": true })];
    match project.loader.kind {
        LoaderKind::NeoForge => {
            components.push(json!({ "uid": "net.neoforged", "version": loader }))
        }
        LoaderKind::Forge => {
            components.push(json!({ "uid": "net.minecraftforge", "version": loader }))
        }
        LoaderKind::Fabric => {
            components.push(json!({ "uid": "net.fabricmc.intermediary", "version": mc }));
            components.push(json!({ "uid": "net.fabricmc.fabric-loader", "version": loader }));
        }
        LoaderKind::Quilt => {
            components.push(json!({ "uid": "net.fabricmc.intermediary", "version": mc }));
            components.push(json!({ "uid": "org.quiltmc.quilt-loader", "version": loader }));
        }
    }
    let pack = json!({ "components": components, "formatVersion": 1 });
    serde_json::to_string_pretty(&pack).expect("json values serialize")
}

/// `4G`, `512M` → megabytes.
fn megabytes(size: &str) -> Option<u64> {
    let size = size.trim();
    let (number, unit) = size.split_at(size.find(|c: char| !c.is_ascii_digit())?);
    let number: u64 = number.parse().ok()?;
    match unit.to_ascii_uppercase().as_str() {
        "G" | "GB" => Some(number * 1024),
        "M" | "MB" => Some(number),
        _ => None,
    }
}

fn prism_instance(project: &Project) -> String {
    let mut cfg = format!("InstanceType=OneSix\nname={}\n", project.name);
    if let Some(memory) = &project.java.memory
        && let (Some(min), Some(max)) = (megabytes(&memory.min), megabytes(&memory.recommended))
    {
        cfg.push_str(&format!(
            "OverrideMemory=true\nMinMemAlloc={min}\nMaxMemAlloc={max}\n"
        ));
    }
    if !project.java.jvm_args.is_empty() {
        cfg.push_str(&format!(
            "OverrideJavaArgs=true\nJvmArgs={}\n",
            project.java.jvm_args.join(" ")
        ));
    }
    cfg
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use super::*;

    fn project() -> Project {
        riven_format::from_str(
            r#"{"format":1,"name":"Pack","id":"pack","version":"1.0.0","minecraft":"1.21.1",
                "loader":{"type":"neoforge","version":"21.1.77"},
                "java":{"major":21,"memory":{"min":"4G","recommended":"8G"}},
                "groups":[{"id":"shaders","name":"Shaders","default":false}],
                "content":[
                 {"id":"sodium","kind":"mod","name":"Sodium",
                  "source":{"type":"modrinth","project":"AANobbMI","version":"v1"},
                  "file":{"path":"mods/sodium.jar","size":3,
                    "hashes":{"sha1":"0000000000000000000000000000000000000000","sha512":"00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"},
                    "url":"https://cdn.modrinth.com/data/AANobbMI/versions/v1/sodium.jar"},
                  "side":"client","update":"follow","reason":"explicit"},
                 {"id":"iris","kind":"mod","name":"Iris",
                  "source":{"type":"modrinth","project":"YL57xq9U","version":"v2"},
                  "file":{"path":"mods/iris.jar","size":3,
                    "hashes":{"sha1":"2222222222222222222222222222222222222222","sha512":"22222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222222"},
                    "url":"https://cdn.modrinth.com/data/YL57xq9U/versions/v2/iris.jar"},
                  "side":"client","group":"shaders","update":"follow","reason":"explicit"},
                 {"id":"jei","kind":"mod","name":"JEI",
                  "source":{"type":"url","url":"https://example.org/jei.jar"},
                  "file":{"path":"mods/jei.jar","size":3,"hashes":{"sha1":"1111111111111111111111111111111111111111"},
                    "url":"https://example.org/jei.jar"},
                  "side":"both","update":"follow","reason":"explicit"},
                 {"id":"proxy","kind":"mod","name":"Proxy",
                  "source":{"type":"url","url":"https://example.org/proxy.jar"},
                  "file":{"path":"mods/proxy.jar","size":3,"hashes":{"sha1":"4444444444444444444444444444444444444444"},
                    "url":"https://example.org/proxy.jar"},
                  "side":"server","update":"pinned","reason":"explicit"}]}"#,
        )
        .unwrap()
    }

    fn entries(path: &Path) -> Vec<String> {
        let zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        let mut names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        names
    }

    fn text(path: &Path, name: &str) -> String {
        let bytes = std::fs::read(path).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut out = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut out).unwrap();
        out
    }

    fn setup(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("riven-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("overrides/common/config")).unwrap();
        std::fs::create_dir_all(dir.join("overrides/server")).unwrap();
        std::fs::write(dir.join("overrides/common/config/a.toml"), "a").unwrap();
        std::fs::write(dir.join("overrides/server/server.properties"), "motd").unwrap();
        dir
    }

    fn bytes(entry: &Entry) -> Result<Vec<u8>, String> {
        Ok(entry.id.as_bytes().to_vec())
    }

    #[test]
    fn mrpack_links_allowed_hosts_and_embeds_the_rest() {
        let dir = setup("mrpack");
        let out = dir.join("pack.mrpack");
        let report = export(Format::Mrpack, &project(), &dir, None, &bytes, &out).unwrap();
        assert_eq!(report.linked, 2);
        assert_eq!(report.embedded, 2);
        assert!(report.skipped.is_empty());
        assert_eq!(
            entries(&out),
            [
                "modrinth.index.json",
                "overrides/config/a.toml",
                "overrides/mods/jei.jar",
                "server-overrides/mods/proxy.jar",
                "server-overrides/server.properties",
            ]
        );
        let index: serde_json::Value =
            serde_json::from_str(&text(&out, "modrinth.index.json")).unwrap();
        assert_eq!(index["dependencies"]["neoforge"], "21.1.77");
        let iris = &index["files"][1];
        assert_eq!(iris["path"], "mods/iris.jar");
        assert_eq!(iris["env"]["client"], "optional");
        assert_eq!(iris["env"]["server"], "unsupported");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prism_targets_the_client() {
        let dir = setup("prism");
        let out = dir.join("prism.zip");
        let report = export(Format::Prism, &project(), &dir, None, &bytes, &out).unwrap();
        assert_eq!(report.embedded, 2);
        assert_eq!(
            entries(&out),
            [
                ".minecraft/config/a.toml",
                ".minecraft/mods/jei.jar",
                ".minecraft/mods/sodium.jar",
                "instance.cfg",
                "mmc-pack.json",
            ]
        );
        assert!(text(&out, "instance.cfg").contains("MaxMemAlloc=8192"));
        assert!(text(&out, "mmc-pack.json").contains("net.neoforged"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
