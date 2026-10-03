use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};

use riven_format::{
    Entry, EntryFile, FileRules, Group, Hashes, Java, Kind, Loader, PackPath, PathError, Project,
    Reason, Side, Source, UpdatePolicy,
};
use riven_sources::{DependencyKind, Known, Matched};

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
        let name = file.name().to_owned();
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
