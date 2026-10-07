use std::collections::HashMap;
use std::future::Future;

use futures_util::{StreamExt, TryStreamExt, stream};
use riven_format::{Loader, LoaderKind, PackPath, Side};
use riven_sources::Known;
use serde::Deserialize;
use sha2::Digest as _;

use crate::import::{Error, Override, PackFile, PackInfo, Scope};

const PARALLEL_READS: usize = 16;

#[derive(Debug, Deserialize)]
struct PackToml {
    name: String,
    author: Option<String>,
    version: Option<String>,
    description: Option<String>,
    index: IndexRef,
    #[serde(default)]
    versions: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct IndexRef {
    file: String,
    hash_format: String,
    hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct IndexToml {
    hash_format: String,
    #[serde(default)]
    files: Vec<IndexFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct IndexFile {
    file: String,
    hash: String,
    hash_format: Option<String>,
    alias: Option<String>,
    #[serde(default)]
    metafile: bool,
    #[serde(default)]
    preserve: bool,
}

#[derive(Debug, Deserialize)]
struct MetaFile {
    name: String,
    filename: String,
    side: Option<String>,
    download: Download,
    #[serde(default)]
    update: Update,
    option: Option<OptionalFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Download {
    url: Option<String>,
    hash_format: String,
    hash: String,
}

#[derive(Debug, Default, Deserialize)]
struct Update {
    curseforge: Option<CurseForgeFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct CurseForgeFile {
    file_id: u64,
}

#[derive(Debug, Deserialize)]
struct OptionalFile {
    #[serde(default)]
    optional: bool,
}

/// A packwiz pack: `pack.toml`, its index, metafiles and the files they list.
#[derive(Debug, Clone)]
pub struct Packwiz {
    pub info: PackInfo,
    pub overrides: Vec<Override>,
    pub files: Vec<PackFile>,
    /// Paths the index marks `preserve`.
    pub preserve: Vec<String>,
}

/// The CurseForge CDN address of a file, which serves it without an API key.
pub fn curseforge_cdn(file_id: u64, filename: &str) -> String {
    let mut url = url::Url::parse("https://edge.forgecdn.net/").expect("static URL parses");
    url.path_segments_mut()
        .expect("http URLs have a path")
        .pop_if_empty()
        .extend([
            "files",
            &(file_id / 1000).to_string(),
            &(file_id % 1000).to_string(),
            filename,
        ]);
    url.into()
}

fn parse<T: serde::de::DeserializeOwned>(file: &str, bytes: &[u8]) -> Result<T, Error> {
    let read_err = |message: String| Error::Read {
        file: file.to_owned(),
        message,
    };
    let text = std::str::from_utf8(bytes).map_err(|e| read_err(e.to_string()))?;
    toml::from_str(text).map_err(|e| read_err(e.to_string()))
}

fn verify(file: &str, bytes: &[u8], format: &str, expected: &str) -> Result<(), Error> {
    let actual = match format {
        "sha256" => hex::encode(sha2::Sha256::digest(bytes)),
        "sha512" => hex::encode(sha2::Sha512::digest(bytes)),
        "sha1" => hex::encode(sha1::Sha1::digest(bytes)),
        _ => return Ok(()),
    };
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::Read {
            file: file.to_owned(),
            message: format!("{format} does not match the index"),
        })
    }
}

/// `dir/name` with `dir` possibly empty.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn loader(versions: &HashMap<String, String>) -> Result<Loader, Error> {
    [
        ("neoforge", LoaderKind::NeoForge),
        ("forge", LoaderKind::Forge),
        ("fabric", LoaderKind::Fabric),
        ("quilt", LoaderKind::Quilt),
    ]
    .into_iter()
    .find_map(|(key, kind)| {
        versions.get(key).map(|version| Loader {
            kind,
            version: version.clone(),
        })
    })
    .ok_or_else(|| Error::Unsupported("packs without a mod loader".into()))
}

fn pack_file(meta_path: &str, meta: MetaFile) -> Result<PackFile, Error> {
    let path = PackPath::new(join(parent(meta_path), &meta.filename))?;
    let hash = Some(meta.download.hash.to_ascii_lowercase());
    let known = match meta.download.hash_format.as_str() {
        "sha512" => Known {
            sha512: hash,
            sha1: None,
        },
        "sha1" => Known {
            sha512: None,
            sha1: hash,
        },
        _ => Known::default(),
    };
    let download = meta.download.url.or_else(|| {
        meta.update
            .curseforge
            .map(|cf| curseforge_cdn(cf.file_id, &meta.filename))
    });
    Ok(PackFile {
        path,
        known,
        size: 0,
        side: match meta.side.as_deref() {
            Some("client") => Some(Side::Client),
            Some("server") => Some(Side::Server),
            _ => Some(Side::Both),
        },
        downloads: download.into_iter().collect(),
        archive_name: None,
        optional: meta.option.is_some_and(|o| o.optional),
        title: Some(meta.name),
    })
}

impl Packwiz {
    /// Reads a pack through `load`, which returns a file by its path relative to `pack.toml`.
    pub async fn read<F, Fut>(load: F) -> Result<Self, Error>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<Vec<u8>, String>>,
    {
        let fetch = |file: String| {
            let read = load(file.clone());
            async move {
                read.await
                    .map(|bytes| (file.clone(), bytes))
                    .map_err(|message| Error::Read { file, message })
            }
        };
        let (_, bytes) = fetch("pack.toml".into()).await?;
        let pack: PackToml = parse("pack.toml", &bytes)?;
        let index_path = PackPath::new(pack.index.file.as_str())?;
        let (_, bytes) = fetch(index_path.to_string()).await?;
        verify(
            index_path.as_str(),
            &bytes,
            &pack.index.hash_format,
            &pack.index.hash,
        )?;
        let index: IndexToml = parse(index_path.as_str(), &bytes)?;
        let base = parent(index_path.as_str());

        let listed: Vec<(String, &IndexFile)> = index
            .files
            .iter()
            .map(|f| Ok((join(base, PackPath::new(f.file.as_str())?.as_str()), f)))
            .collect::<Result<_, Error>>()?;
        let paths: Vec<String> = listed.iter().map(|(p, _)| p.clone()).collect();
        let loaded: HashMap<String, Vec<u8>> = stream::iter(paths)
            .map(fetch)
            .buffered(PARALLEL_READS)
            .try_collect()
            .await?;

        let mut packwiz = Packwiz {
            info: PackInfo {
                name: pack.name,
                version: pack.version.unwrap_or_else(|| "1.0.0".into()),
                summary: pack.description,
                authors: pack.author.into_iter().collect(),
                minecraft: pack
                    .versions
                    .get("minecraft")
                    .cloned()
                    .ok_or_else(|| Error::Unsupported("no minecraft version".into()))?,
                loader: loader(&pack.versions)?,
            },
            overrides: vec![],
            files: vec![],
            preserve: vec![],
        };
        for (full, entry) in &listed {
            let bytes = &loaded[full];
            let format = entry.hash_format.as_deref().unwrap_or(&index.hash_format);
            verify(full, bytes, format, &entry.hash)?;
            if entry.metafile {
                let meta: MetaFile = parse(full, bytes)?;
                packwiz.files.push(pack_file(&entry.file, meta)?);
                continue;
            }
            let path = PackPath::new(entry.alias.as_deref().unwrap_or(&entry.file))?;
            if entry.preserve {
                packwiz.preserve.push(path.to_string());
            }
            packwiz.overrides.push(Override {
                scope: Scope::Common,
                path,
                bytes: bytes.clone(),
            });
        }
        Ok(packwiz)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256(text: &str) -> String {
        hex::encode(sha2::Sha256::digest(text.as_bytes()))
    }

    fn pack(files: &[(&str, &str, &str)]) -> HashMap<String, Vec<u8>> {
        let mut index = String::from("hash-format = \"sha256\"\n");
        let mut all = HashMap::new();
        for (path, extra, body) in files {
            index.push_str(&format!(
                "[[files]]\nfile = \"{path}\"\nhash = \"{}\"\n{extra}\n",
                sha256(body)
            ));
            all.insert(path.to_string(), body.as_bytes().to_vec());
        }
        let pack = format!(
            "name = \"Pack\"\nauthor = \"BX Team\"\nversion = \"1.6.0\"\n\
             [index]\nfile = \"index.toml\"\nhash-format = \"sha256\"\nhash = \"{}\"\n\
             [versions]\nminecraft = \"1.21.1\"\nneoforge = \"21.1.236\"\n",
            sha256(&index)
        );
        all.insert("index.toml".into(), index.into_bytes());
        all.insert("pack.toml".into(), pack.into_bytes());
        all
    }

    async fn read(files: HashMap<String, Vec<u8>>) -> Result<Packwiz, Error> {
        Packwiz::read(|path| {
            let found = files.get(&path).cloned().ok_or(format!("{path} missing"));
            async move { found }
        })
        .await
    }

    const MODRINTH: &str = r#"name = "Better Ores"
filename = "§7Better Ores.zip"
side = "client"
[download]
url = "https://cdn.modrinth.com/data/9D4BBjDX/versions/AE4ECGoO/x.zip"
hash-format = "sha512"
hash = "C14F"
[update.modrinth]
mod-id = "9D4BBjDX"
version = "AE4ECGoO"
"#;

    const CURSEFORGE: &str = r#"name = "Jade"
filename = "Jade 1.21.1+neo.jar"
side = "both"
[download]
hash-format = "sha1"
hash = "88ee316e68900080b017f60c12162e2731924cf8"
mode = "metadata:curseforge"
[update.curseforge]
file-id = 8591019
project-id = 324717
[option]
optional = true
"#;

    #[tokio::test]
    async fn metafiles_become_content_and_the_rest_overrides() {
        let pw = read(pack(&[
            (
                "resourcepacks/better-ores.pw.toml",
                "metafile = true",
                MODRINTH,
            ),
            ("mods/jade.pw.toml", "metafile = true", CURSEFORGE),
            ("config/a.toml", "", "a = 1"),
            ("options.txt", "preserve = true", "fov:90"),
        ]))
        .await
        .unwrap();
        assert_eq!(pw.info.loader.kind, LoaderKind::NeoForge);
        assert_eq!(pw.info.authors, ["BX Team"]);

        let ores = &pw.files[0];
        assert_eq!(ores.path.as_str(), "resourcepacks/§7Better Ores.zip");
        assert_eq!(ores.known.sha512.as_deref(), Some("c14f"));
        assert_eq!(ores.side, Some(Side::Client));

        let jade = &pw.files[1];
        assert_eq!(jade.path.as_str(), "mods/Jade 1.21.1+neo.jar");
        assert_eq!(
            jade.downloads,
            ["https://edge.forgecdn.net/files/8591/19/Jade%201.21.1+neo.jar"]
        );
        assert!(jade.optional);

        let overrides: Vec<&str> = pw.overrides.iter().map(|o| o.path.as_str()).collect();
        assert_eq!(overrides, ["config/a.toml", "options.txt"]);
        assert_eq!(pw.preserve, ["options.txt"]);
    }

    #[tokio::test]
    async fn tampered_files_and_escaping_paths_are_refused() {
        let mut files = pack(&[("config/a.toml", "", "a = 1")]);
        files.insert("config/a.toml".into(), b"a = 2".to_vec());
        assert!(matches!(read(files).await, Err(Error::Read { .. })));

        let files = pack(&[("../outside.txt", "", "x")]);
        assert!(matches!(read(files).await, Err(Error::Path(_))));
    }
}
