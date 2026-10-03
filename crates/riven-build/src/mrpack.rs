use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};

use riven_format::{
    Entry, EntryFile, FileRules, Hashes, Java, Kind, Loader, LoaderKind, PackPath, PathError,
    Project, Reason, Side, Source, UpdatePolicy,
};
use riven_sources::{DependencyKind, ProjectInfo, Version};
use serde::Deserialize;

const INDEX: &str = "modrinth.index.json";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a valid .mrpack: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("cannot read {file}: {message}")]
    Read { file: String, message: String },
    #[error("unsupported .mrpack: {0}")]
    Unsupported(String),
    #[error("unsafe path in .mrpack: {0}")]
    Path(#[from] PathError),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub format_version: u32,
    pub game: String,
    pub version_id: String,
    pub name: String,
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<IndexFile>,
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexFile {
    pub path: String,
    pub hashes: IndexHashes,
    pub env: Option<Env>,
    #[serde(default)]
    pub downloads: Vec<String>,
    pub file_size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexHashes {
    pub sha1: Option<String>,
    pub sha512: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Env {
    pub client: Option<String>,
    pub server: Option<String>,
}

/// Which `overrides/` subdirectory a file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Common,
    Client,
    Server,
}

impl Scope {
    pub fn dir(self) -> &'static str {
        match self {
            Scope::Common => "common",
            Scope::Client => "client",
            Scope::Server => "server",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Override {
    pub scope: Scope,
    pub path: PackPath,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Mrpack {
    pub index: Index,
    pub overrides: Vec<Override>,
    /// Mods and packs exporters embedded in overrides instead of linking them.
    pub embedded: Vec<IndexFile>,
}

/// `riven.json` built from a pack, plus embedded files no platform could identify.
#[derive(Debug, Clone)]
pub struct Imported {
    pub project: Project,
    pub unmatched: Vec<PackPath>,
}

/// Whether an override is content (a jar or pack zip) rather than configuration.
fn is_content(path: &PackPath) -> bool {
    let p = path.as_str();
    let dir = p.split('/').next().unwrap_or("");
    matches!(dir, "mods" | "resourcepacks" | "shaderpacks")
        && p.matches('/').count() == 1
        && (p.ends_with(".jar") || p.ends_with(".zip"))
}

fn embedded_file(scope: Scope, path: PackPath, bytes: &[u8]) -> IndexFile {
    use sha1::Digest as _;
    let env = |client: &str, server: &str| {
        Some(Env {
            client: Some(client.into()),
            server: Some(server.into()),
        })
    };
    IndexFile {
        path: path.as_str().to_owned(),
        hashes: IndexHashes {
            sha1: Some(hex::encode(sha1::Sha1::digest(bytes))),
            sha512: Some(hex::encode(sha2::Sha512::digest(bytes))),
        },
        env: match scope {
            Scope::Common => None,
            Scope::Client => env("required", "unsupported"),
            Scope::Server => env("unsupported", "required"),
        },
        downloads: vec![],
        file_size: bytes.len() as u64,
    }
}

impl Mrpack {
    pub fn read(bytes: &[u8]) -> Result<Self, Error> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        let read_err = |file: &str, message: String| Error::Read {
            file: file.to_owned(),
            message,
        };

        let mut text = String::new();
        zip.by_name(INDEX)
            .map_err(|_| Error::Unsupported(format!("no {INDEX}")))?
            .read_to_string(&mut text)
            .map_err(|e| read_err(INDEX, e.to_string()))?;
        let index: Index =
            serde_json::from_str(&text).map_err(|e| read_err(INDEX, e.to_string()))?;
        if index.format_version != 1 || index.game != "minecraft" {
            return Err(Error::Unsupported(format!(
                "format {} for game `{}`",
                index.format_version, index.game
            )));
        }
        for file in &index.files {
            PackPath::new(file.path.as_str())?;
        }

        let mut overrides = Vec::new();
        let mut embedded = Vec::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().to_owned();
            let Some((scope, rest)) = [
                ("overrides/", Scope::Common),
                ("client-overrides/", Scope::Client),
                ("server-overrides/", Scope::Server),
            ]
            .into_iter()
            .find_map(|(prefix, scope)| name.strip_prefix(prefix).map(|rest| (scope, rest))) else {
                continue;
            };
            let path = PackPath::new(rest)?;
            let mut bytes = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut bytes)
                .map_err(|e| read_err(&name, e.to_string()))?;
            if is_content(&path) {
                embedded.push(embedded_file(scope, path, &bytes));
            } else {
                overrides.push(Override { scope, path, bytes });
            }
        }
        Ok(Self {
            index,
            overrides,
            embedded,
        })
    }

    pub fn loader(&self) -> Result<Loader, Error> {
        [
            ("neoforge", LoaderKind::NeoForge),
            ("forge", LoaderKind::Forge),
            ("fabric-loader", LoaderKind::Fabric),
            ("quilt-loader", LoaderKind::Quilt),
        ]
        .into_iter()
        .find_map(|(key, kind)| {
            self.index.dependencies.get(key).map(|version| Loader {
                kind,
                version: version.clone(),
            })
        })
        .ok_or_else(|| Error::Unsupported("packs without a mod loader".into()))
    }

    pub fn minecraft(&self) -> Result<&str, Error> {
        self.index
            .dependencies
            .get("minecraft")
            .map(String::as_str)
            .ok_or_else(|| Error::Unsupported("no minecraft version".into()))
    }

    /// sha512 hashes of all listed files, for a batch Modrinth lookup.
    pub fn sha512s(&self) -> Vec<String> {
        self.index
            .files
            .iter()
            .chain(&self.embedded)
            .filter_map(|f| f.hashes.sha512.clone())
            .collect()
    }

    /// Builds `riven.json` from the index and what Modrinth knows about its files.
    pub fn to_project(
        &self,
        versions: &HashMap<String, Version>,
        projects: &HashMap<String, ProjectInfo>,
        java: u32,
    ) -> Result<Imported, Error> {
        let mut unmatched = Vec::new();
        let mut taken = HashSet::new();
        let mut by_project: HashMap<String, String> = HashMap::new();
        let mut content = Vec::new();
        let mut deps_of: Vec<(usize, Vec<String>)> = Vec::new();

        let listed = self.index.files.iter().map(|f| (f, false));
        let embedded = self.embedded.iter().map(|f| (f, true));
        for (file, is_embedded) in listed.chain(embedded) {
            let path = PackPath::new(file.path.as_str())?;
            let sha512 = file.hashes.sha512.as_ref();
            let version = sha512.and_then(|h| versions.get(h));
            if is_embedded && version.is_none() {
                unmatched.push(path);
                continue;
            }
            // Embedded files have no download list; take the platform's url for this exact file.
            let url = file.downloads.first().cloned().or_else(|| {
                version?
                    .files
                    .iter()
                    .find(|f| f.hashes.sha512.as_ref() == sha512)?
                    .url
                    .clone()
            });
            let info = version.and_then(|v| projects.get(&v.project));
            let stem = path.file_name().trim_end_matches(".jar");
            let id = unique_id(info.map_or(stem, |i| i.slug.as_str()), &mut taken);
            let source = match version {
                Some(v) => Source::Modrinth {
                    project: v.project.clone(),
                    version: v.id.clone(),
                },
                None => Source::Url {
                    url: file.downloads.first().cloned().ok_or_else(|| {
                        Error::Unsupported(format!("`{path}` has no download url"))
                    })?,
                },
            };
            if let Some(v) = version {
                by_project.insert(v.project.clone(), id.clone());
                let required = v
                    .dependencies
                    .iter()
                    .filter(|d| d.kind == DependencyKind::Required)
                    .filter_map(|d| d.project.clone())
                    .collect();
                deps_of.push((content.len(), required));
            }
            content.push(Entry {
                id,
                kind: kind_of(&path),
                name: info.map_or_else(|| path.file_name().to_owned(), |i| i.title.clone()),
                update: if version.is_some() {
                    UpdatePolicy::Follow
                } else {
                    UpdatePolicy::Pinned
                },
                source,
                file: EntryFile {
                    path,
                    size: file.file_size,
                    hashes: Hashes {
                        sha512: file.hashes.sha512.clone(),
                        sha1: file.hashes.sha1.clone(),
                    },
                    url,
                },
                side: side_of(file.env.as_ref()),
                group: None,
                reason: Reason::Explicit,
                requires: vec![],
            });
        }

        for (index, projects) in deps_of {
            let mut requires: Vec<String> = projects
                .iter()
                .filter_map(|p| by_project.get(p).cloned())
                .filter(|id| *id != content[index].id)
                .collect();
            requires.sort();
            requires.dedup();
            content[index].requires = requires;
        }
        let required: HashSet<String> = content
            .iter()
            .flat_map(|e| e.requires.iter().cloned())
            .collect();
        for entry in &mut content {
            if required.contains(&entry.id) {
                entry.reason = Reason::Dependency;
            }
        }

        let project = Project {
            id: unique_id(&self.index.name, &mut HashSet::new()),
            name: self.index.name.clone(),
            version: self.index.version_id.clone(),
            authors: vec![],
            description: self.index.summary.clone().filter(|s| !s.is_empty()),
            icon: None,
            minecraft: self.minecraft()?.to_owned(),
            loader: self.loader()?,
            java: Java {
                major: java,
                memory: None,
                jvm_args: vec![],
            },
            groups: vec![],
            files: FileRules::default(),
            content,
        };
        Ok(Imported { project, unmatched })
    }
}

fn kind_of(path: &PackPath) -> Kind {
    match path.as_str().split('/').next() {
        Some("mods") => Kind::Mod,
        Some("resourcepacks") => Kind::ResourcePack,
        Some("shaderpacks") => Kind::ShaderPack,
        Some("datapacks") => Kind::DataPack,
        _ => Kind::File,
    }
}

fn side_of(env: Option<&Env>) -> Side {
    let unsupported = |v: &Option<String>| v.as_deref() == Some("unsupported");
    match env {
        Some(env) if unsupported(&env.client) => Side::Server,
        Some(env) if unsupported(&env.server) => Side::Client,
        _ => Side::Both,
    }
}

/// A lowercase slug not yet in `taken` (`name`, `name-2`, …).
fn unique_id(name: &str, taken: &mut HashSet<String>) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let base = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() {
        "entry".into()
    } else {
        base
    };
    let id = (1..)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|id| !taken.contains(id))
        .expect("some suffix is free");
    taken.insert(id.clone());
    id
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use riven_sources::{Channel, Dependency, Support};

    use super::*;

    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        for (name, body) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    fn sha(c: char) -> String {
        c.to_string().repeat(128)
    }

    fn index() -> String {
        format!(
            r#"{{
  "formatVersion": 1, "game": "minecraft", "versionId": "1.4.0", "name": "VideCraft: Create",
  "summary": "",
  "files": [
    {{ "path": "mods/create.jar", "hashes": {{ "sha1": "{s1}", "sha512": "{a}" }},
       "env": {{ "client": "required", "server": "required" }},
       "downloads": ["https://cdn.modrinth.com/create.jar"], "fileSize": 10 }},
    {{ "path": "mods/ponder.jar", "hashes": {{ "sha1": "{s1}", "sha512": "{b}" }},
       "downloads": ["https://cdn.modrinth.com/ponder.jar"], "fileSize": 20 }},
    {{ "path": "mods/sodium.jar", "hashes": {{ "sha1": "{s1}", "sha512": "{c}" }},
       "env": {{ "client": "required", "server": "unsupported" }},
       "downloads": ["https://cdn.modrinth.com/sodium.jar"], "fileSize": 30 }},
    {{ "path": "mods/easylogin.jar", "hashes": {{ "sha1": "{s1}", "sha512": "{d}" }},
       "env": {{ "client": "unsupported", "server": "required" }},
       "downloads": ["https://github.com/x/easylogin.jar"], "fileSize": 40 }}
  ],
  "dependencies": {{ "minecraft": "1.21.1", "neoforge": "21.1.77" }}
}}"#,
            s1 = "f".repeat(40),
            a = sha('a'),
            b = sha('b'),
            c = sha('c'),
            d = sha('d'),
        )
    }

    fn version(id: &str, project: &str, deps: &[&str]) -> Version {
        Version {
            id: id.into(),
            project: project.into(),
            name: id.into(),
            number: id.into(),
            channel: Channel::Release,
            game_versions: vec![],
            loaders: vec![],
            published: String::new(),
            files: vec![],
            dependencies: deps
                .iter()
                .map(|p| Dependency {
                    project: Some((*p).into()),
                    version: None,
                    kind: DependencyKind::Required,
                })
                .collect(),
        }
    }

    fn project(id: &str, slug: &str) -> ProjectInfo {
        ProjectInfo {
            id: id.into(),
            slug: slug.into(),
            title: slug.into(),
            description: String::new(),
            kind: Some(Kind::Mod),
            client: Support::Required,
            server: Support::Required,
            owner: String::new(),
        }
    }

    #[test]
    fn imports_files_overrides_and_dependency_reasons() {
        let bytes = zip(&[
            (INDEX, index().as_bytes()),
            ("overrides/config/create-common.toml", b"a = 1"),
            ("overrides/kubejs/", b""),
            ("client-overrides/options.txt", b"fov:90"),
            ("server-overrides/server.properties", b"motd=hi"),
            ("overrides/mods/jei.jar", b"jei bytes"),
            ("client-overrides/mods/private.jar", b"private bytes"),
        ]);
        let pack = Mrpack::read(&bytes).unwrap();
        let scopes: Vec<_> = pack
            .overrides
            .iter()
            .map(|o| (o.scope, o.path.as_str()))
            .collect();
        assert_eq!(
            scopes,
            [
                (Scope::Common, "config/create-common.toml"),
                (Scope::Client, "options.txt"),
                (Scope::Server, "server.properties"),
            ]
        );

        assert_eq!(pack.embedded.len(), 2);
        let jei_sha = pack.embedded[0].hashes.sha512.clone().unwrap();
        let mut jei = version("j1", "J", &[]);
        jei.files.push(riven_sources::VersionFile {
            filename: "jei.jar".into(),
            url: Some("https://cdn.modrinth.com/jei.jar".into()),
            size: 9,
            hashes: Hashes {
                sha512: Some(jei_sha.clone()),
                ..Hashes::default()
            },
            primary: true,
        });
        let versions = HashMap::from([
            (jei_sha, jei),
            (sha('a'), version("c1", "C", &["P"])),
            (sha('b'), version("p1", "P", &[])),
            (sha('c'), version("s1", "S", &[])),
        ]);
        let projects = HashMap::from([
            ("C".into(), project("C", "create")),
            ("P".into(), project("P", "ponder")),
            ("S".into(), project("S", "sodium")),
            ("J".into(), project("J", "jei")),
        ]);
        let imported = pack.to_project(&versions, &projects, 21).unwrap();
        assert_eq!(
            imported.unmatched,
            [PackPath::new("mods/private.jar").unwrap()]
        );
        let project = imported.project;
        assert_eq!(
            project.entry("jei").unwrap().file.url.as_deref(),
            Some("https://cdn.modrinth.com/jei.jar")
        );
        assert_eq!(project.id, "videcraft-create");
        assert_eq!(project.loader.kind, LoaderKind::NeoForge);
        assert_eq!(project.description, None);
        assert!(project.validate().is_empty(), "{:?}", project.validate());

        let entry = |id: &str| project.entry(id).unwrap();
        assert_eq!(entry("create").requires, ["ponder"]);
        assert_eq!(entry("create").reason, Reason::Explicit);
        assert_eq!(entry("ponder").reason, Reason::Dependency);
        assert_eq!(entry("sodium").side, Side::Client);
        assert_eq!(entry("easylogin").side, Side::Server);
        assert!(
            matches!(&entry("easylogin").source, Source::Url { url } if url.contains("github"))
        );
        assert_eq!(entry("easylogin").update, UpdatePolicy::Pinned);
    }

    #[test]
    fn rejects_traversal_in_overrides_and_files() {
        let evil_override = zip(&[
            (INDEX, index().as_bytes()),
            ("overrides/../../.bashrc", b"x"),
        ]);
        assert!(matches!(Mrpack::read(&evil_override), Err(Error::Path(_))));
        let evil_file = index().replace("mods/create.jar", "../mods/create.jar");
        assert!(matches!(
            Mrpack::read(&zip(&[(INDEX, evil_file.as_bytes())])),
            Err(Error::Path(_))
        ));
    }
}
