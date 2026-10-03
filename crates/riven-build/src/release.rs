use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use riven_format::{
    Channel, Entry, KeyPair, PackPath, PathError, Project, PublicKey, Release, ReleaseFile, Side,
    SignError, Source,
};
use sha2::{Digest, Sha512};

use crate::glob;
use crate::import::Scope;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("unsafe path {0}")]
    Path(#[from] PathError),
    #[error("{} is a symlink; overrides must be plain files", .0.display())]
    Symlink(PathBuf),
    #[error("`{0}` has no download url")]
    MissingUrl(String),
    #[error("`{id}`: {path} changed since it was added; run `riven update {id}`")]
    LocalChanged { id: String, path: String },
    #[error(
        "release {0} is already built with different contents; releases are immutable, bump the version"
    )]
    ReleaseExists(String),
    #[error("no release {0} in dist/; build it first")]
    NoRelease(String),
    #[error("`{0}` cannot be used as a file name")]
    BadName(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Sign(#[from] SignError),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

fn sha512_hex(bytes: &[u8]) -> String {
    hex::encode(Sha512::digest(bytes))
}

/// The relative URL a manifest uses for a blob.
pub fn blob_url(sha512: &str) -> String {
    format!("../blobs/{}/{sha512}", &sha512[..2])
}

/// A file of the repository the release serves itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob {
    pub sha512: String,
    pub size: u64,
    pub source: PathBuf,
}

/// A file under `overrides/<scope>/`.
#[derive(Debug, Clone)]
pub struct OverrideFile {
    pub scope: Scope,
    pub path: PackPath,
    pub source: PathBuf,
}

fn walk(
    dir: &Path,
    rel: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, PathBuf)>,
) -> Result<(), Error> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(io(dir))?
        .collect::<Result<_, _>>()
        .map_err(io(dir))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type().map_err(io(&path))?;
        rel.push(entry.file_name().to_string_lossy().into_owned());
        if kind.is_symlink() {
            return Err(Error::Symlink(path));
        } else if kind.is_dir() {
            walk(&path, rel, out)?;
        } else {
            out.push((rel.clone(), path));
        }
        rel.pop();
    }
    Ok(())
}

/// Files under `overrides/{common,client,server}`, minus `files.ignore` and local entries' sources.
pub fn collect_overrides(repo: &Path, project: &Project) -> Result<Vec<OverrideFile>, Error> {
    let local: Vec<&str> = project
        .content
        .iter()
        .filter_map(|e| match &e.source {
            Source::Local { path } => Some(path.as_str()),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for scope in [Scope::Common, Scope::Client, Scope::Server] {
        let dir = repo.join("overrides").join(scope.dir());
        if !dir.is_dir() {
            continue;
        }
        let mut found = Vec::new();
        walk(&dir, &mut Vec::new(), &mut found)?;
        for (parts, source) in found {
            let path = PackPath::new(parts.join("/"))?;
            let in_repo = format!("overrides/{}/{path}", scope.dir());
            if glob::any(&project.files.ignore, path.as_str()) || local.contains(&in_repo.as_str())
            {
                continue;
            }
            out.push(OverrideFile {
                scope,
                path,
                source,
            });
        }
    }
    Ok(out)
}

/// A release manifest and the blobs it references.
#[derive(Debug, Clone)]
pub struct Built {
    pub release: Release,
    pub blobs: Vec<Blob>,
}

fn blob_file(
    path: PackPath,
    blob: &Blob,
    side: Side,
    group: Option<String>,
    preserve: bool,
) -> ReleaseFile {
    ReleaseFile {
        path,
        size: blob.size,
        hashes: riven_format::Hashes {
            sha512: Some(blob.sha512.clone()),
            ..Default::default()
        },
        urls: vec![blob_url(&blob.sha512)],
        side,
        group,
        preserve,
    }
}

fn read_blob(source: &Path) -> Result<Blob, Error> {
    let bytes = std::fs::read(source).map_err(io(source))?;
    Ok(Blob {
        sha512: sha512_hex(&bytes),
        size: bytes.len() as u64,
        source: source.to_owned(),
    })
}

/// Builds the manifest of `project` at its current version.
pub fn build_release(project: &Project, repo: &Path) -> Result<Built, Error> {
    if let Some(issue) = project.validate().first() {
        return Err(Error::Invalid(issue.to_string()));
    }
    let mut files = Vec::new();
    let mut blobs: BTreeMap<String, Blob> = BTreeMap::new();
    let preserve = |path: &PackPath| glob::any(&project.files.preserve, path.as_str());

    for entry in &project.content {
        let Entry { file, .. } = entry;
        match &entry.source {
            Source::Local { path } => {
                let blob = read_blob(&repo.join(path.as_str()))?;
                if file
                    .hashes
                    .sha512
                    .as_ref()
                    .is_some_and(|h| *h != blob.sha512)
                {
                    return Err(Error::LocalChanged {
                        id: entry.id.clone(),
                        path: path.to_string(),
                    });
                }
                let p = file.path.clone();
                let keep = preserve(&p);
                files.push(blob_file(p, &blob, entry.side, entry.group.clone(), keep));
                blobs.insert(blob.sha512.clone(), blob);
            }
            _ => files.push(ReleaseFile {
                path: file.path.clone(),
                size: file.size,
                hashes: file.hashes.clone(),
                urls: vec![
                    file.url
                        .clone()
                        .ok_or_else(|| Error::MissingUrl(entry.id.clone()))?,
                ],
                side: entry.side,
                group: entry.group.clone(),
                preserve: preserve(&file.path),
            }),
        }
    }

    let mut by_path: BTreeMap<PackPath, [Option<Blob>; 3]> = BTreeMap::new();
    for file in collect_overrides(repo, project)? {
        let slot = match file.scope {
            Scope::Common => 0,
            Scope::Client => 1,
            Scope::Server => 2,
        };
        by_path.entry(file.path).or_default()[slot] = Some(read_blob(&file.source)?);
    }
    let taken: Vec<PackPath> = files.iter().map(|f| f.path.clone()).collect();
    for (path, [common, client, server]) in by_path {
        if taken.contains(&path) {
            return Err(Error::Invalid(format!(
                "overrides and a content entry both install `{path}`"
            )));
        }
        let keep = preserve(&path);
        let for_client = client.or_else(|| common.clone());
        let for_server = server.or(common);
        match (for_client, for_server) {
            (Some(c), Some(s)) if c.sha512 == s.sha512 => {
                files.push(blob_file(path, &c, Side::Both, None, keep));
                blobs.insert(c.sha512.clone(), c);
            }
            (c, s) => {
                for (blob, side) in [(c, Side::Client), (s, Side::Server)] {
                    if let Some(blob) = blob {
                        files.push(blob_file(path.clone(), &blob, side, None, keep));
                        blobs.insert(blob.sha512.clone(), blob);
                    }
                }
            }
        }
    }

    Ok(Built {
        release: Release {
            id: project.id.clone(),
            name: project.name.clone(),
            version: project.version.clone(),
            minecraft: project.minecraft.clone(),
            loader: project.loader.clone(),
            java: project.java.clone(),
            groups: project.groups.clone(),
            files,
        },
        blobs: blobs.into_values().collect(),
    })
}

fn safe_name(name: &str) -> Result<&str, Error> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
    if ok {
        Ok(name)
    } else {
        Err(Error::BadName(name.to_owned()))
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().expect("dist paths have a parent");
    std::fs::create_dir_all(parent).map_err(io(parent))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
}

fn sig_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".sig");
    PathBuf::from(name)
}

/// Writes blobs and `releases/<version>.json` (+ `.sig`) under `dist`; built releases never change.
pub fn write_release(dist: &Path, built: &Built, key: Option<&KeyPair>) -> Result<(), Error> {
    let version = safe_name(&built.release.version)?;
    let manifest = riven_format::to_string(&built.release);
    let path = dist.join("releases").join(format!("{version}.json"));
    match std::fs::read_to_string(&path) {
        Ok(existing) if existing != manifest => {
            return Err(Error::ReleaseExists(version.to_owned()));
        }
        _ => {}
    }
    for blob in &built.blobs {
        let target = dist.join(blob_url(&blob.sha512).trim_start_matches("../"));
        if !target.is_file() {
            let parent = target.parent().expect("blob paths have a parent");
            std::fs::create_dir_all(parent).map_err(io(parent))?;
            std::fs::copy(&blob.source, &target).map_err(io(&target))?;
        }
    }
    write_file(&path, manifest.as_bytes())?;
    if let Some(key) = key {
        write_file(&sig_path(&path), key.sign(manifest.as_bytes()).as_bytes())?;
    }
    Ok(())
}

/// Points `channels/<channel>.json` at an already built release; also how rollbacks happen.
pub fn publish(
    dist: &Path,
    channel: &str,
    version: &str,
    key: Option<&KeyPair>,
) -> Result<Channel, Error> {
    let channel = safe_name(channel)?;
    let version = safe_name(version)?;
    let manifest_path = dist.join("releases").join(format!("{version}.json"));
    let manifest = std::fs::read(&manifest_path).map_err(|_| Error::NoRelease(version.into()))?;
    if let Some(key) = key {
        let sig = std::fs::read_to_string(sig_path(&manifest_path)).unwrap_or_default();
        if key.public().verify(&manifest, &sig).is_err() {
            return Err(Error::Invalid(format!(
                "release {version} is not signed with this key; rebuild it or publish with the key that built it"
            )));
        }
    }
    let pointer = Channel {
        version: version.to_owned(),
        manifest: format!("../releases/{version}.json"),
        sha512: sha512_hex(&manifest),
    };
    let text = riven_format::to_string(&pointer);
    let path = dist.join("channels").join(format!("{channel}.json"));
    write_file(&path, text.as_bytes())?;
    let sig = sig_path(&path);
    match key {
        Some(key) => write_file(&sig, key.sign(text.as_bytes()).as_bytes())?,
        None => {
            let _ = std::fs::remove_file(&sig);
        }
    }
    Ok(pointer)
}

/// Verifies a document against its `.sig` text with `key`.
pub fn verify(key: &PublicKey, bytes: &[u8], sig: &str) -> Result<(), Error> {
    Ok(key.verify(bytes, sig)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("riven-build-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(repo: &Path, path: &str, body: &str) {
        let path = repo.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn project() -> Project {
        let mut project: Project = riven_format::from_str(
            r#"{"format":1,"name":"Pack","id":"pack","version":"1.0.0","minecraft":"1.21.1",
                "loader":{"type":"neoforge","version":"21.1.77"},"java":{"major":21},
                "files":{"preserve":["options.txt"],"ignore":["**/*.bak"]},
                "content":[{"id":"sodium","kind":"mod","name":"Sodium",
                  "source":{"type":"modrinth","project":"AANobbMI","version":"v1"},
                  "file":{"path":"mods/sodium.jar","size":3,"hashes":{"sha1":"0000000000000000000000000000000000000000"},
                          "url":"https://cdn.modrinth.com/sodium.jar"},
                  "side":"client","update":"follow","reason":"explicit"}]}"#,
        )
        .unwrap();
        project.content.sort_by(|a, b| a.id.cmp(&b.id));
        project
    }

    #[test]
    fn overrides_split_by_side_and_skip_ignored() {
        let repo = tempdir("sides");
        put(&repo, "overrides/common/config/a.toml", "same");
        put(&repo, "overrides/common/config/b.toml", "common");
        put(&repo, "overrides/client/config/b.toml", "client");
        put(&repo, "overrides/server/server.properties", "motd=x");
        put(&repo, "overrides/client/options.txt", "fov:90");
        put(&repo, "overrides/common/config/old.bak", "x");
        let built = build_release(&project(), &repo).unwrap();
        let files: Vec<(&str, Side, bool)> = built
            .release
            .files
            .iter()
            .filter(|f| f.urls[0].starts_with("../blobs/"))
            .map(|f| (f.path.as_str(), f.side, f.preserve))
            .collect();
        assert_eq!(
            files,
            [
                ("config/a.toml", Side::Both, false),
                ("config/b.toml", Side::Client, false),
                ("config/b.toml", Side::Server, false),
                ("options.txt", Side::Client, true),
                ("server.properties", Side::Server, false),
            ]
        );
        assert_eq!(built.blobs.len(), 5);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn releases_are_immutable_and_channels_signed() {
        let repo = tempdir("immutable");
        let dist = repo.join("dist");
        put(&repo, "overrides/common/config/a.toml", "v1");
        let key = KeyPair::generate().unwrap();
        let built = build_release(&project(), &repo).unwrap();
        write_release(&dist, &built, Some(&key)).unwrap();
        write_release(&dist, &built, Some(&key)).unwrap();

        put(&repo, "overrides/common/config/a.toml", "v2");
        let changed = build_release(&project(), &repo).unwrap();
        assert!(matches!(
            write_release(&dist, &changed, Some(&key)),
            Err(Error::ReleaseExists(_))
        ));

        let pointer = publish(&dist, "stable", "1.0.0", Some(&key)).unwrap();
        let text = std::fs::read(dist.join("channels/stable.json")).unwrap();
        let sig = std::fs::read_to_string(dist.join("channels/stable.json.sig")).unwrap();
        verify(&key.public(), &text, &sig).unwrap();
        let manifest = std::fs::read(dist.join("releases/1.0.0.json")).unwrap();
        assert_eq!(pointer.sha512, sha512_hex(&manifest));

        let stranger = KeyPair::generate().unwrap();
        assert!(publish(&dist, "stable", "1.0.0", Some(&stranger)).is_err());
        assert!(matches!(
            publish(&dist, "beta", "9.9.9", Some(&key)),
            Err(Error::NoRelease(_))
        ));
        assert!(matches!(
            publish(&dist, "../escape", "1.0.0", None),
            Err(Error::BadName(_))
        ));
        let _ = std::fs::remove_dir_all(repo);
    }
}
