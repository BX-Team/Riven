use std::collections::HashMap;
use std::io::{Cursor, Read};

use riven_format::{Loader, LoaderKind, PackPath, Side};
use riven_sources::Known;
use serde::Deserialize;

use crate::import::{Error, Override, PackFile, PackInfo, Scope, read_overrides};

const INDEX: &str = "modrinth.index.json";

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

/// A Modrinth `.mrpack` archive.
#[derive(Debug, Clone)]
pub struct Mrpack {
    pub index: Index,
    pub overrides: Vec<Override>,
    /// Listed files, then content exporters embedded in overrides instead of linking it.
    pub files: Vec<PackFile>,
}

fn side_of(env: Option<&Env>) -> Option<Side> {
    let unsupported = |v: &Option<String>| v.as_deref() == Some("unsupported");
    match env {
        Some(env) if unsupported(&env.client) => Some(Side::Server),
        Some(env) if unsupported(&env.server) => Some(Side::Client),
        _ => Some(Side::Both),
    }
}

impl Mrpack {
    pub fn read(bytes: &[u8]) -> Result<Self, Error> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        let read_err = |message: String| Error::Read {
            file: INDEX.to_owned(),
            message,
        };
        let mut text = String::new();
        zip.by_name(INDEX)
            .map_err(|_| Error::Unsupported(format!("no {INDEX}")))?
            .read_to_string(&mut text)
            .map_err(|e| read_err(e.to_string()))?;
        let index: Index = serde_json::from_str(&text).map_err(|e| read_err(e.to_string()))?;
        if index.format_version != 1 || index.game != "minecraft" {
            return Err(Error::Unsupported(format!(
                "format {} for game `{}`",
                index.format_version, index.game
            )));
        }
        let mut files = Vec::new();
        for file in &index.files {
            files.push(PackFile {
                path: PackPath::new(file.path.as_str())?,
                known: Known {
                    sha512: file.hashes.sha512.clone(),
                    sha1: file.hashes.sha1.clone(),
                },
                size: file.file_size,
                side: side_of(file.env.as_ref()),
                downloads: file.downloads.clone(),
                archive_name: None,
                optional: false,
                title: None,
            });
        }
        let (overrides, embedded) = read_overrides(
            bytes,
            &[
                ("overrides/", Scope::Common),
                ("client-overrides/", Scope::Client),
                ("server-overrides/", Scope::Server),
            ],
        )?;
        files.extend(embedded);
        Ok(Self {
            index,
            overrides,
            files,
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

    pub fn info(&self) -> Result<PackInfo, Error> {
        Ok(PackInfo {
            name: self.index.name.clone(),
            version: self.index.version_id.clone(),
            summary: self.index.summary.clone(),
            authors: vec![],
            minecraft: self.minecraft()?.to_owned(),
            loader: self.loader()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use riven_format::{Hashes, Kind, Reason, Source, UpdatePolicy};
    use riven_sources::{
        Channel, Dependency, DependencyKind, Matched, ProjectInfo, Support, Version,
    };

    use super::*;
    use crate::import::{Resolution, build_project};

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
            icon_url: None,
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

        assert_eq!(pack.files.len(), 6);
        let jei_sha = pack.files[4].known.sha512.clone().unwrap();
        assert_eq!(pack.files[5].side, Some(Side::Client));
        assert!(pack.files[5].known.sha1.is_some());
        let mut jei = version("j1", "J", &[]);
        jei.files.push(riven_sources::VersionFile {
            filename: "jei.jar".into(),
            url: Some("https://cdn.modrinth.com/jei.jar".into()),
            size: 9,
            hashes: Hashes {
                sha512: Some(jei_sha),
                ..Hashes::default()
            },
            primary: true,
        });
        let on_modrinth = |version: Version, slug: &str| {
            Resolution::Platform(Box::new(Matched {
                source: Source::Modrinth {
                    project: version.project.clone(),
                    version: version.id.clone(),
                },
                info: project(&version.project, slug),
                version,
            }))
        };
        let resolved = [
            on_modrinth(version("c1", "C", &["P"]), "create"),
            on_modrinth(version("p1", "P", &[]), "ponder"),
            on_modrinth(version("s1", "S", &[]), "sodium"),
            Resolution::Url("https://github.com/x/easylogin.jar".into()),
            on_modrinth(jei, "jei"),
            Resolution::Local(PackPath::new("local/private.jar").unwrap()),
        ];
        let project = build_project(&pack.info().unwrap(), 21, &pack.files, &resolved).unwrap();
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
        let private = entry("private");
        assert!(
            matches!(&private.source, Source::Local { path } if path.as_str() == "local/private.jar")
        );
        assert_eq!(private.file.path.as_str(), "mods/private.jar");
        assert_eq!(private.side, Side::Client);
        assert_eq!(private.file.url, None);
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
