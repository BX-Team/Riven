use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use futures_util::{StreamExt, stream};

use riven_format::{
    Entry, EntryFile, FileRules, Group, Hashes, Java, Kind, Loader, PackPath, PathError, Project,
    Reason, Side, Source, UpdatePolicy,
};
use riven_resolve::Downloader;
use riven_sources::{Cache, DependencyKind, Known, Matched, Modrinth};

use crate::author::AuthorError;
use crate::mrpack::Mrpack;
use crate::packwiz::Packwiz;
use crate::workspace::{
    OVERRIDES, PROJECT_FILE, StoreJars, Workspace, ensure_gitignore, game_meta, write_atomic,
};

/// The group optional files of an imported pack go to.
pub const OPTIONAL_GROUP: &str = "optional";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a valid archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("cannot read {file}: {message}")]
    Read { file: String, message: String },
    #[error("unsupported modpack: {0}")]
    Unsupported(String),
    #[error("unsafe path in the modpack: {0}")]
    Path(#[from] PathError),
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

/// A configuration file the archive carries.
#[derive(Debug, Clone)]
pub struct Override {
    pub scope: Scope,
    pub path: PackPath,
    pub bytes: Vec<u8>,
}

/// A content file of a modpack, before Modrinth identifies it.
#[derive(Debug, Clone)]
pub struct PackFile {
    pub path: PackPath,
    pub known: Known,
    pub size: u64,
    /// The side the archive states, if it states one.
    pub side: Option<Side>,
    pub downloads: Vec<String>,
    /// The entry name inside the archive, for files it embeds.
    pub archive_name: Option<String>,
    pub optional: bool,
    /// The display name the pack gives the file, if any.
    pub title: Option<String>,
}

/// Pack-level facts read from an archive.
#[derive(Debug, Clone)]
pub struct PackInfo {
    pub name: String,
    pub version: String,
    pub summary: Option<String>,
    pub authors: Vec<String>,
    pub minecraft: String,
    pub loader: Loader,
}

/// What an archive file becomes in `riven.json`.
#[derive(Debug, Clone)]
pub enum Resolution {
    Platform(Box<Matched>),
    Url(String),
    /// A copy kept in the repository at this path.
    Local(PackPath),
    Skipped,
}

/// Whether an archive file is content (a jar or pack zip) rather than configuration.
pub(crate) fn is_content(path: &PackPath) -> bool {
    let p = path.as_str();
    let dir = p.split('/').next().unwrap_or("");
    matches!(dir, "mods" | "resourcepacks" | "shaderpacks")
        && p.matches('/').count() == 1
        && (p.ends_with(".jar") || p.ends_with(".zip"))
}

/// A [`PackFile`] for content the archive embeds, hashed for Modrinth lookups.
pub(crate) fn embedded_file(
    path: PackPath,
    archive_name: String,
    side: Option<Side>,
    bytes: &[u8],
) -> PackFile {
    use sha1::Digest as _;
    PackFile {
        path,
        known: Known {
            sha512: Some(hex::encode(sha2::Sha512::digest(bytes))),
            sha1: Some(hex::encode(sha1::Sha1::digest(bytes))),
        },
        size: bytes.len() as u64,
        side,
        downloads: vec![],
        archive_name: Some(archive_name),
        optional: false,
        title: None,
    }
}

/// The bytes of `name` inside a zip archive.
pub fn extract(archive: &[u8], name: &str) -> Result<Vec<u8>, Error> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
    let mut file = zip.by_name(name)?;
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes).map_err(|e| Error::Read {
        file: name.to_owned(),
        message: e.to_string(),
    })?;
    Ok(bytes)
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

/// A lowercase slug not yet in `taken` (`name`, `name-2`, …).
pub(crate) fn unique_id(name: &str, taken: &mut HashSet<String>) -> String {
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

/// Builds `riven.json` from an archive's files and how each was resolved.
pub fn build_project(
    info: &PackInfo,
    java: u32,
    files: &[PackFile],
    resolved: &[Resolution],
) -> Result<Project, Error> {
    let mut taken = HashSet::new();
    let mut by_project: HashMap<String, String> = HashMap::new();
    let mut content = Vec::new();
    let mut deps_of: Vec<(usize, Vec<String>)> = Vec::new();

    for (file, resolution) in files.iter().zip(resolved) {
        let matched = match resolution {
            Resolution::Skipped => continue,
            Resolution::Platform(m) => Some(m.as_ref()),
            _ => None,
        };
        let platform_file = matched.and_then(|m| {
            let version = &m.version;
            version
                .files
                .iter()
                .find(|f| {
                    (f.hashes.sha512.is_some() && f.hashes.sha512 == file.known.sha512)
                        || (f.hashes.sha1.is_some() && f.hashes.sha1 == file.known.sha1)
                })
                .or_else(|| version.primary_file())
        });
        let path = file.path.clone();
        let stem = path
            .file_name()
            .trim_end_matches(".jar")
            .trim_end_matches(".zip")
            .to_owned();
        let label = matched
            .map(|m| m.info.slug.as_str())
            .or(file.title.as_deref())
            .unwrap_or(&stem);
        let id = unique_id(label, &mut taken);
        let source = match resolution {
            Resolution::Platform(m) => m.source.clone(),
            Resolution::Url(url) => Source::Url { url: url.clone() },
            Resolution::Local(local) => Source::Local {
                path: local.clone(),
            },
            Resolution::Skipped => unreachable!("skipped above"),
        };
        if let Some(m) = matched {
            by_project.insert(m.version.project.clone(), id.clone());
            let required = m
                .version
                .dependencies
                .iter()
                .filter(|d| d.kind == DependencyKind::Required)
                .filter_map(|d| d.project.clone())
                .collect();
            deps_of.push((content.len(), required));
        }
        let pf_hashes = platform_file.map(|f| f.hashes.clone()).unwrap_or_default();
        let hashes = Hashes {
            sha512: file.known.sha512.clone().or(pf_hashes.sha512),
            sha1: file.known.sha1.clone().or(pf_hashes.sha1),
        };
        let url = match resolution {
            Resolution::Local(_) => None,
            _ => platform_file
                .and_then(|f| f.url.clone())
                .or_else(|| file.downloads.first().cloned()),
        };
        // Client-only mods crash dedicated servers, but a "server-only" mod missing from
        // clients breaks joining and platforms often mislabel it: trust only client sides.
        let side = file
            .side
            .unwrap_or_else(|| match matched.map(|m| m.info.side()) {
                Some(Side::Client) => Side::Client,
                _ => Side::Both,
            });
        content.push(Entry {
            id,
            kind: matched
                .and_then(|m| m.info.kind)
                .unwrap_or_else(|| kind_of(&path)),
            name: matched
                .map(|m| m.info.title.clone())
                .or_else(|| file.title.clone())
                .unwrap_or_else(|| path.file_name().to_owned()),
            update: match resolution {
                Resolution::Url(_) => UpdatePolicy::Pinned,
                _ => UpdatePolicy::Follow,
            },
            source,
            file: EntryFile {
                size: if file.size > 0 {
                    file.size
                } else {
                    platform_file.map_or(0, |f| f.size)
                },
                path,
                hashes,
                url,
            },
            side,
            group: file.optional.then(|| OPTIONAL_GROUP.to_owned()),
            reason: Reason::Explicit,
            requires: vec![],
        });
    }

    for (index, projects) in deps_of {
        let mut requires: Vec<String> = projects
            .into_iter()
            .filter_map(|p| by_project.get(&p).cloned())
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

    let groups = if content.iter().any(|e| e.group.is_some()) {
        vec![Group {
            id: OPTIONAL_GROUP.into(),
            name: "Optional".into(),
            description: Some("Files the original pack marked as optional".into()),
            default: false,
        }]
    } else {
        vec![]
    };
    Ok(Project {
        id: unique_id(&info.name, &mut HashSet::new()),
        name: info.name.clone(),
        version: info.version.clone(),
        authors: info.authors.clone(),
        description: info.summary.clone().filter(|s| !s.is_empty()),
        icon: None,
        minecraft: info.minecraft.clone(),
        loader: info.loader.clone(),
        java: Java {
            major: java,
            memory: None,
            jvm_args: vec![],
        },
        groups,
        files: FileRules::default(),
        content,
    })
}

/// Reads an archive's overrides under `prefixes`, splitting embedded content from configuration.
pub(crate) fn read_overrides(
    archive: &[u8],
    prefixes: &[(&str, Scope)],
) -> Result<(Vec<Override>, Vec<PackFile>), Error> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))?;
    let mut overrides = Vec::new();
    let mut embedded = Vec::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name()?.into_owned();
        let Some((scope, rest)) = prefixes
            .iter()
            .find_map(|(prefix, scope)| name.strip_prefix(prefix).map(|rest| (*scope, rest)))
        else {
            continue;
        };
        let path = PackPath::new(rest)?;
        let mut bytes = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut bytes).map_err(|e| Error::Read {
            file: name.clone(),
            message: e.to_string(),
        })?;
        if is_content(&path) {
            let side = match scope {
                Scope::Common => None,
                Scope::Client => Some(Side::Client),
                Scope::Server => Some(Side::Server),
            };
            embedded.push(embedded_file(path, name, side, &bytes));
        } else {
            overrides.push(Override { scope, path, bytes });
        }
    }
    Ok((overrides, embedded))
}

const PARALLEL_DOWNLOADS: usize = 8;

/// Pack formats `riven import` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportKind {
    Mrpack,
    /// A packwiz pack: its directory, `pack.toml`, or the URL of `pack.toml`.
    Packwiz,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(transparent)]
    Pack(#[from] Error),
    #[error(transparent)]
    Author(#[from] AuthorError),
    #[error(transparent)]
    Source(#[from] riven_sources::Error),
    #[error("cannot download {url}: {message}")]
    Download { url: String, message: String },
    #[error("{} already exists", .0.display())]
    Exists(PathBuf),
    #[error("{url} does not match the sha1 the pack lists for {path}")]
    Mismatch { url: String, path: PackPath },
    #[error("bad URL {0}")]
    BadUrl(String),
}

/// How far an import got, for a spinner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportStage {
    Reading,
    Identifying { files: usize },
    Downloading,
    Writing,
}

/// A pack imported into a fresh `riven.json`.
#[derive(Debug)]
pub struct Imported {
    pub workspace: Workspace,
    pub overrides: usize,
    /// Pack paths of files that could not be identified and were left out.
    pub unmatched: Vec<String>,
}

fn is_http(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

async fn read_source(source: &str) -> Result<Vec<u8>, ImportError> {
    if is_http(source) {
        let failed = |e: reqwest::Error| ImportError::Download {
            url: source.to_owned(),
            message: e.to_string(),
        };
        let response = riven_sources::client()
            .get(source)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(failed)?;
        Ok(response.bytes().await.map_err(failed)?.to_vec())
    } else {
        std::fs::read(source).map_err(|e| crate::workspace::io(Path::new(source))(e).into())
    }
}

/// Reads a packwiz pack from a directory, a `pack.toml` path, or a `pack.toml` URL.
async fn read_packwiz(source: &str) -> Result<Packwiz, ImportError> {
    if is_http(source) {
        let mut base =
            url::Url::parse(source).map_err(|_| ImportError::BadUrl(source.to_owned()))?;
        if !base.path().ends_with(".toml") && !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let http = riven_sources::client();
        let pack = Packwiz::read(|path| {
            let url = base.join(&path);
            let http = http.clone();
            async move {
                let url = url.map_err(|e| e.to_string())?;
                let response = http
                    .get(url)
                    .send()
                    .await
                    .and_then(reqwest::Response::error_for_status)
                    .map_err(|e| e.to_string())?;
                let bytes = response.bytes().await.map_err(|e| e.to_string())?;
                Ok(bytes.to_vec())
            }
        })
        .await?;
        return Ok(pack);
    }
    let path = Path::new(source);
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(Path::new("."))
    };
    let pack = Packwiz::read(|file| {
        let path = dir.join(file);
        async move { std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display())) }
    })
    .await?;
    Ok(pack)
}

/// Downloads files kept as URL sources whose sha512 or size the pack does not record.
async fn complete_urls(
    jars: &StoreJars,
    files: &mut [PackFile],
    resolved: &[Resolution],
) -> Result<(), ImportError> {
    let wanted: Vec<(usize, String)> = resolved
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match r {
            Resolution::Url(url) if files[i].known.sha512.is_none() || files[i].size == 0 => {
                Some((i, url.clone()))
            }
            _ => None,
        })
        .collect();
    let fetched: Vec<_> = stream::iter(wanted)
        .map(|(i, url)| async move { (i, url.clone(), jars.download(&url).await) })
        .buffer_unordered(PARALLEL_DOWNLOADS)
        .collect()
        .await;
    for (i, url, result) in fetched {
        let file = &mut files[i];
        let got = result.map_err(|message| ImportError::Download {
            url: url.clone(),
            message,
        })?;
        if let Some(sha1) = &file.known.sha1
            && got.hashes.sha1.as_deref() != Some(sha1.as_str())
        {
            return Err(ImportError::Mismatch {
                url,
                path: file.path.clone(),
            });
        }
        file.known.sha512 = got.hashes.sha512;
        file.known.sha1 = got.hashes.sha1;
        file.size = got.size;
    }
    Ok(())
}

/// A `local/` path for an unidentified embedded file, unique among those taken.
fn local_copy(file: &PackFile, taken: &mut HashSet<String>) -> Result<PackPath, ImportError> {
    let name = file.path.file_name().to_owned();
    let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
    let candidate = (1..)
        .map(|n| match n {
            1 => name.clone(),
            n => format!("{stem}-{n}.{ext}"),
        })
        .find(|c| !taken.contains(c))
        .expect("some suffix is free");
    taken.insert(candidate.clone());
    Ok(PackPath::new(format!("local/{candidate}")).map_err(Error::Path)?)
}

/// Turns a modpack into `dir/riven.json`, `overrides/` and `local/`, identifying files on Modrinth.
pub async fn import_pack(
    dir: &Path,
    kind: ImportKind,
    source: &str,
    stage: impl Fn(ImportStage),
) -> Result<Imported, ImportError> {
    let project_path = dir.join(PROJECT_FILE);
    if project_path.exists() {
        return Err(ImportError::Exists(project_path));
    }
    stage(ImportStage::Reading);
    let (archive, info, overrides, mut files, preserve) = match kind {
        ImportKind::Mrpack => {
            let bytes = read_source(source).await?;
            let pack = Mrpack::read(&bytes)?;
            (
                Some(bytes),
                pack.info()?,
                pack.overrides,
                pack.files,
                vec![],
            )
        }
        ImportKind::Packwiz => {
            let pack = read_packwiz(source).await?;
            (None, pack.info, pack.overrides, pack.files, pack.preserve)
        }
    };

    std::fs::create_dir_all(dir).map_err(crate::workspace::io(dir))?;
    let cache = Cache::new(
        dir.join(".riven").join("cache"),
        std::time::Duration::from_secs(300),
    );
    let modrinth = Modrinth::new(riven_sources::client()).with_cache(cache.clone());
    stage(ImportStage::Identifying { files: files.len() });
    let known: Vec<Known> = files.iter().map(|f| f.known.clone()).collect();
    let matched = riven_sources::identify(&modrinth, &known).await?;
    let java = game_meta(Some(cache)).java_major(&info.minecraft).await?;

    let mut unmatched = Vec::new();
    let mut taken = HashSet::new();
    let mut copies = Vec::new();
    let mut resolved = Vec::new();
    for (file, found) in files.iter().zip(matched) {
        resolved.push(match found {
            Some(m) => Resolution::Platform(Box::new(m)),
            None if file.archive_name.is_some() => {
                let local = local_copy(file, &mut taken)?;
                copies.push((file.archive_name.clone().unwrap_or_default(), local.clone()));
                Resolution::Local(local)
            }
            None if !file.downloads.is_empty() => Resolution::Url(file.downloads[0].clone()),
            None => {
                unmatched.push(file.path.to_string());
                Resolution::Skipped
            }
        });
    }
    if resolved.iter().any(|r| matches!(r, Resolution::Url(_))) {
        stage(ImportStage::Downloading);
        complete_urls(&StoreJars::new(dir)?, &mut files, &resolved).await?;
    }
    let mut project = build_project(&info, java, &files, &resolved)?;
    project.files.preserve = preserve;

    stage(ImportStage::Writing);
    let write = |target: &Path, bytes: &[u8]| -> Result<(), ImportError> {
        if target.exists() {
            return Err(ImportError::Exists(target.to_owned()));
        }
        let parent = target.parent().expect("pack paths have a parent");
        std::fs::create_dir_all(parent).map_err(crate::workspace::io(parent))?;
        std::fs::write(target, bytes).map_err(|e| crate::workspace::io(target)(e).into())
    };
    for file in &overrides {
        let target = dir
            .join(OVERRIDES)
            .join(file.scope.dir())
            .join(file.path.as_str());
        write(&target, &file.bytes)?;
    }
    for (archive_name, local) in &copies {
        let archive = archive
            .as_deref()
            .expect("only archives embed files to copy");
        write(&dir.join(local.as_str()), &extract(archive, archive_name)?)?;
    }
    write_atomic(&project_path, &riven_format::to_string(&project))?;
    ensure_gitignore(dir)?;
    Ok(Imported {
        workspace: Workspace {
            dir: dir.to_owned(),
            project,
        },
        overrides: overrides.len(),
        unmatched,
    })
}
