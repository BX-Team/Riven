use std::path::{Path, PathBuf};

use futures_util::{StreamExt, stream};
use riven_format::{
    Entry, Group, Kind, PackPath, Project, Side, Source as EntrySource, SourceKind, UpdatePolicy,
};
use riven_resolve::{
    AddOptions, Downloader, Env, Installed, JarFetcher, JarMeta, ModVersion, Plan, Planner,
    ResolveError, carry_over, direct_entry, github_update, rehome,
};
use riven_sources::{Known, Modrinth, Source, Target};

use crate::workspace::{StoreJars, Workspace, io};

const PARALLEL_DOWNLOADS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum AuthorError {
    #[error("no riven.json in {} or any parent directory; run `riven init`", .0.display())]
    NoProject(PathBuf),
    #[error("{} already exists", .0.display())]
    Exists(PathBuf),
    #[error("cannot locate the data directory")]
    NoDataDir,
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: {source}", path.display())]
    Format {
        path: PathBuf,
        source: riven_format::Error,
    },
    #[error("refusing to write an invalid riven.json: {0}")]
    Invalid(String),
    #[error(transparent)]
    Source(#[from] riven_sources::Error),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Path(#[from] riven_format::PathError),
    #[error("cannot download {url}: {message}")]
    Download { url: String, message: String },
    #[error("cannot store {path}: {message}")]
    Store { path: String, message: String },
    #[error("`{0}` is not a Modrinth project or version URL")]
    BadModrinthUrl(String),
    #[error("`{0}` is not an http(s) URL")]
    NotHttp(String),
    #[error("`{0}` is not a GitHub repository, release or release asset URL")]
    BadGitHub(String),
    #[error("`{0}` does not end in a file name")]
    NoFileName(String),
    #[error("{} is not a file", .0.display())]
    NotAFile(PathBuf),
    #[error("{} already exists with other contents", .0.display())]
    LocalClash(PathBuf),
    #[error("keep local files under local/ or overrides/ (got {})", .0.display())]
    LocalOutside(PathBuf),
    #[error("nothing on Modrinth matches `{query}` for Minecraft {minecraft}")]
    NoMatch { query: String, minecraft: String },
    #[error(
        "`{query}` is ambiguous; add one by slug:\n{}",
        options.iter().map(|(slug, title)| format!("  {slug} — {title}")).collect::<Vec<_>>().join("\n")
    )]
    Ambiguous {
        query: String,
        /// `(slug, title)` of each candidate.
        options: Vec<(String, String)>,
    },
    #[error("--asset only applies to GitHub sources")]
    AssetNotGitHub,
    #[error("`{0}` is not a URL entry; --url only repoints URL sources")]
    NotUrlEntry(String),
    #[error("--url repoints one entry: riven update <id> --url <url>")]
    UrlNeedsOneEntry,
    #[error("no entry `{0}` in the pack")]
    NoEntry(String),
    #[error("no group `{0}`; create it with `riven group add {0} --name …`")]
    NoGroup(String),
    #[error("group `{0}` already exists")]
    GroupExists(String),
    #[error("version `{0}` is not MAJOR.MINOR.PATCH; pass an explicit version")]
    NotSemver(String),
    #[error("empty version")]
    EmptyVersion,
    #[error("`{0}` is not a file under overrides/")]
    NotOverride(PackPath),
    #[error("`{0}` is not a text file")]
    Binary(PackPath),
}

/// Turns a slug, id, Modrinth URL or search query into a project id and an optional version.
pub async fn find_project(
    modrinth: &Modrinth,
    pack: &Project,
    query: &str,
) -> Result<(String, Option<String>), AuthorError> {
    if let Ok(url) = url::Url::parse(query)
        && url.host_str().is_some_and(|h| h.ends_with("modrinth.com"))
    {
        let segments: Vec<&str> = url
            .path_segments()
            .map(|s| s.filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        return match segments.as_slice() {
            [_, slug] => Ok(((*slug).to_owned(), None)),
            [_, slug, "version", version, ..] | ["data", slug, "versions", version, ..] => {
                Ok(((*slug).to_owned(), Some((*version).to_owned())))
            }
            _ => Err(AuthorError::BadModrinthUrl(query.to_owned())),
        };
    }
    match modrinth.project(query).await {
        Ok(info) => return Ok((info.id, None)),
        Err(riven_sources::Error::NotFound(_)) => {}
        Err(e) => return Err(e.into()),
    }
    let target = Target {
        minecraft: pack.minecraft.clone(),
        loader: pack.loader.kind,
        kind: Kind::Mod,
    };
    let hits = modrinth.search(query, &target, 5).await?;
    let exact = hits
        .iter()
        .find(|h| h.slug.eq_ignore_ascii_case(query) || h.title.eq_ignore_ascii_case(query));
    match (exact, hits.as_slice()) {
        (Some(hit), _) | (None, [hit]) => Ok((hit.id.clone(), None)),
        (None, []) => Err(AuthorError::NoMatch {
            query: query.to_owned(),
            minecraft: pack.minecraft.clone(),
        }),
        (None, hits) => Err(AuthorError::Ambiguous {
            query: query.to_owned(),
            options: hits
                .iter()
                .map(|h| (h.slug.clone(), h.title.clone()))
                .collect(),
        }),
    }
}

/// What `riven add` adds and how.
#[derive(Debug, Clone, Default)]
pub struct AddRequest {
    /// Modrinth slug, id, search query or URL; a GitHub repo or release URL; a file URL; a local file.
    pub query: String,
    /// Where `query` points; guessed from it when unset.
    pub source: Option<SourceKind>,
    /// GitHub asset name pattern.
    pub asset: Option<String>,
    pub side: Option<Side>,
    pub group: Option<String>,
    pub pin: bool,
}

/// Where an `add` query points.
enum Origin {
    Modrinth,
    GitHub {
        repo: String,
        tag: Option<String>,
        asset: Option<String>,
    },
    Url(String),
    Local(PathBuf),
}

fn origin(query: &str, source: Option<SourceKind>) -> Result<Origin, AuthorError> {
    let web = url::Url::parse(query)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"));
    let host = web.as_ref().and_then(|u| u.host_str()).unwrap_or_default();
    Ok(match source {
        Some(SourceKind::Modrinth) => Origin::Modrinth,
        Some(SourceKind::GitHub) => github_origin(query)?,
        Some(SourceKind::Url) => match web {
            Some(_) => Origin::Url(query.to_owned()),
            None => return Err(AuthorError::NotHttp(query.to_owned())),
        },
        Some(SourceKind::Local) => Origin::Local(query.into()),
        None if host == "github.com" || host == "www.github.com" => github_origin(query)?,
        None if host.ends_with("modrinth.com") => Origin::Modrinth,
        None if web.is_some() => Origin::Url(query.to_owned()),
        None if Path::new(query).is_file() => Origin::Local(query.into()),
        None => Origin::Modrinth,
    })
}

/// Parses `owner/repo` or a GitHub repository, release or release asset URL.
fn github_origin(query: &str) -> Result<Origin, AuthorError> {
    let bad = || AuthorError::BadGitHub(query.to_owned());
    let path = match url::Url::parse(query) {
        Ok(url) => url.path().to_owned(),
        Err(_) => query.to_owned(),
    };
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (owner, repo, rest) = match segments.as_slice() {
        [owner, repo, rest @ ..] => (*owner, repo.trim_end_matches(".git"), rest),
        _ => return Err(bad()),
    };
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    let (tag, asset) = match rest {
        [] | ["releases"] | ["releases", "latest"] => (None, None),
        ["releases", "tag", tag] => (Some(decode(tag)), None),
        ["releases", "download", tag, asset] => (Some(decode(tag)), Some(decode(asset))),
        _ => return Err(bad()),
    };
    Ok(Origin::GitHub {
        repo: format!("{owner}/{repo}"),
        tag,
        asset,
    })
}

/// The decoded last path segment of `url`, used as the installed file name.
pub fn url_file_name(url: &str) -> Result<String, AuthorError> {
    let parsed = url::Url::parse(url).map_err(|_| AuthorError::NotHttp(url.to_owned()))?;
    parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .map(|s| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8_lossy()
                .into_owned()
        })
        .ok_or_else(|| AuthorError::NoFileName(url.to_owned()))
}

/// The pack path of a local file; files outside the repository are copied into `local/`.
fn local_path(ws: &Workspace, file: &Path) -> Result<PackPath, AuthorError> {
    let file = std::fs::canonicalize(file).map_err(io(file))?;
    if !file.is_file() {
        return Err(AuthorError::NotAFile(file));
    }
    let root = std::fs::canonicalize(&ws.dir).map_err(io(&ws.dir))?;
    let relative = match file.strip_prefix(&root) {
        Ok(relative) => relative.to_owned(),
        Err(_) => {
            let name = file
                .file_name()
                .ok_or_else(|| AuthorError::NotAFile(file.clone()))?;
            let local = ws.dir.join("local");
            let target = local.join(name);
            if target.exists() {
                let same = std::fs::read(&target).map_err(io(&target))?
                    == std::fs::read(&file).map_err(io(&file))?;
                if !same {
                    return Err(AuthorError::LocalClash(target));
                }
            } else {
                std::fs::create_dir_all(&local).map_err(io(&local))?;
                std::fs::copy(&file, &target).map_err(io(&target))?;
            }
            Path::new("local").join(name)
        }
    };
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if !matches!(
        parts.first().map(String::as_str),
        Some("local" | "overrides")
    ) {
        return Err(AuthorError::LocalOutside(relative));
    }
    Ok(PackPath::new(parts.join("/"))?)
}

fn keep(
    jars: &StoreJars,
    bytes: Vec<u8>,
    path: &str,
) -> Result<riven_resolve::Downloaded, AuthorError> {
    jars.keep(bytes, path)
        .map_err(|message| AuthorError::Store {
            path: path.to_owned(),
            message,
        })
}

async fn download(jars: &StoreJars, url: &str) -> Result<riven_resolve::Downloaded, AuthorError> {
    jars.download(url)
        .await
        .map_err(|message| AuthorError::Download {
            url: url.to_owned(),
            message,
        })
}

/// Builds the entry for a GitHub, URL or local origin.
async fn direct_add(
    ws: &Workspace,
    jars: &StoreJars,
    origin: Origin,
    asset: Option<String>,
) -> Result<Entry, AuthorError> {
    let pack = &ws.project;
    Ok(match origin {
        Origin::GitHub {
            repo,
            tag,
            asset: exact,
        } => {
            let pattern = asset.or_else(|| {
                let tag = tag.as_deref().unwrap_or_default();
                exact.map(|name| riven_resolve::asset_pattern(&name, tag, &pack.minecraft))
            });
            riven_resolve::github_entry(
                &ws.github(),
                jars,
                pack,
                &repo,
                tag.as_deref(),
                pattern.as_deref(),
            )
            .await?
        }
        Origin::Url(url) => {
            let name = url_file_name(&url)?;
            let file = download(jars, &url).await?;
            let source = EntrySource::Url { url: url.clone() };
            direct_entry(pack, source, &name, &file, Some(url))?
        }
        Origin::Local(path) => {
            let path = local_path(ws, &path)?;
            let full = ws.dir.join(path.as_str());
            let bytes = std::fs::read(&full).map_err(io(&full))?;
            let file = keep(jars, bytes, path.as_str())?;
            let name = path.file_name().to_owned();
            direct_entry(pack, EntrySource::Local { path }, &name, &file, None)?
        }
        Origin::Modrinth => unreachable!("Modrinth projects go through the planner"),
    })
}

/// Plans adding content and everything it requires; `riven.json` is not touched.
pub async fn plan_add(ws: &Workspace, request: &AddRequest) -> Result<Plan, AuthorError> {
    if let Some(group) = &request.group
        && !ws.project.groups.iter().any(|g| &g.id == group)
    {
        return Err(AuthorError::NoGroup(group.clone()));
    }
    let origin = origin(&request.query, request.source)?;
    if request.asset.is_some() && !matches!(origin, Origin::GitHub { .. }) {
        return Err(AuthorError::AssetNotGitHub);
    }
    let modrinth = ws.modrinth();
    let jars = ws.jars()?;
    if let Origin::Modrinth = origin {
        let (project_id, version) = find_project(&modrinth, &ws.project, &request.query).await?;
        let options = AddOptions {
            version,
            side: request.side,
            group: request.group.clone(),
            pin: request.pin,
        };
        return Ok(Planner::new(&modrinth, &jars, &ws.project)
            .add(&project_id, &options)
            .await?);
    }
    let mut entry = direct_add(ws, &jars, origin, request.asset.clone()).await?;
    if let Some(side) = request.side {
        entry.side = side;
    }
    entry.group = request.group.clone();
    if request.pin {
        entry.update = UpdatePolicy::Pinned;
    }
    Ok(Planner::new(&modrinth, &jars, &ws.project)
        .add_entry(entry)
        .await?)
}

/// Plans removing an entry and, unless `keep_deps`, the dependencies only it needed.
pub fn plan_remove(ws: &Workspace, id: &str, keep_deps: bool) -> Result<Plan, AuthorError> {
    let modrinth = ws.modrinth();
    let jars = ws.jars()?;
    Ok(Planner::new(&modrinth, &jars, &ws.project).remove(id, keep_deps)?)
}

/// Updates the planner cannot find itself: GitHub releases, changed local files, `--url`.
async fn direct_updates(
    ws: &Workspace,
    jars: &StoreJars,
    ids: &[String],
    url: Option<&str>,
) -> Result<(Vec<(Entry, Entry)>, Vec<String>), AuthorError> {
    let pack = &ws.project;
    let github = ws.github();
    let mut updates = Vec::new();
    let mut notes = Vec::new();
    for entry in &pack.content {
        if !ids.is_empty() && !ids.contains(&entry.id) {
            continue;
        }
        if let Some(url) = url {
            if !matches!(entry.source, EntrySource::Url { .. }) {
                return Err(AuthorError::NotUrlEntry(entry.id.clone()));
            }
            let file = download(jars, url).await?;
            let source = EntrySource::Url {
                url: url.to_owned(),
            };
            let new = direct_entry(pack, source, &url_file_name(url)?, &file, Some(url.into()))?;
            updates.push((entry.clone(), carry_over(entry, new)));
            continue;
        }
        if entry.update == UpdatePolicy::Pinned {
            continue;
        }
        match &entry.source {
            EntrySource::GitHub { .. } => {
                if let Some(new) = github_update(&github, jars, pack, entry).await? {
                    updates.push((entry.clone(), new));
                }
            }
            EntrySource::Local { path } => {
                let full = ws.dir.join(path.as_str());
                let bytes = std::fs::read(&full).map_err(io(&full))?;
                let file = keep(jars, bytes, path.as_str())?;
                if file.hashes.sha512 != entry.file.hashes.sha512 {
                    let name = entry.file.path.file_name();
                    let new = direct_entry(pack, entry.source.clone(), name, &file, None)?;
                    updates.push((entry.clone(), carry_over(entry, new)));
                }
            }
            EntrySource::Url { .. } if !ids.is_empty() => notes.push(format!(
                "`{}` is a URL source; repoint it with `riven update {} --url <url>`",
                entry.id, entry.id
            )),
            _ => {}
        }
    }
    Ok((updates, notes))
}

pub fn source_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Modrinth => "Modrinth",
        SourceKind::GitHub => "GitHub",
        SourceKind::Url => "URL",
        SourceKind::Local => "local",
    }
}

/// Entries whose exact file Modrinth also hosts, recorded as Modrinth entries instead.
async fn migrations(
    ws: &Workspace,
    modrinth: &Modrinth,
    ids: &[String],
) -> Result<Vec<(Entry, Entry)>, AuthorError> {
    let candidates: Vec<&Entry> = ws
        .project
        .content
        .iter()
        .filter(|e| ids.is_empty() || ids.contains(&e.id))
        .filter(|e| e.source.kind() != SourceKind::Modrinth)
        .collect();
    if candidates.is_empty() {
        return Ok(vec![]);
    }
    let known: Vec<Known> = candidates
        .iter()
        .map(|e| Known {
            sha512: e.file.hashes.sha512.clone(),
            sha1: e.file.hashes.sha1.clone(),
        })
        .collect();
    let matched = riven_sources::identify(modrinth, &known).await?;
    Ok(candidates
        .into_iter()
        .zip(matched)
        .filter_map(|(entry, m)| Some((entry.clone(), rehome(entry, &m?))))
        .collect())
}

/// Plans updates of every kind of source (all `follow` entries when `ids` is empty).
pub async fn plan_updates(
    ws: &Workspace,
    ids: &[String],
    url: Option<&str>,
) -> Result<Plan, AuthorError> {
    if url.is_some() && ids.len() != 1 {
        return Err(AuthorError::UrlNeedsOneEntry);
    }
    let modrinth = ws.modrinth();
    let jars = ws.jars()?;
    let (mut direct, mut notes) = direct_updates(ws, &jars, ids, url).await?;
    if url.is_none() {
        let moved = migrations(ws, &modrinth, ids).await?;
        for (old, new) in &moved {
            notes.push(format!(
                "`{}` moves from {} to Modrinth (same file)",
                new.id,
                source_name(old.source.kind())
            ));
        }
        direct.retain(|(old, _)| !moved.iter().any(|(m, _)| m.id == old.id));
        direct.extend(moved);
    }
    let mut plan = Planner::new(&modrinth, &jars, &ws.project)
        .with_updates(direct)
        .update(ids)
        .await?;
    plan.notes.extend(notes);
    Ok(plan)
}

fn entry_mut<'a>(project: &'a mut Project, id: &str) -> Result<&'a mut Entry, AuthorError> {
    project
        .content
        .iter_mut()
        .find(|e| e.id == id)
        .ok_or_else(|| AuthorError::NoEntry(id.to_owned()))
}

pub fn set_pinned(project: &mut Project, id: &str, pinned: bool) -> Result<(), AuthorError> {
    entry_mut(project, id)?.update = if pinned {
        UpdatePolicy::Pinned
    } else {
        UpdatePolicy::Follow
    };
    Ok(())
}

pub fn set_side(project: &mut Project, id: &str, side: Side) -> Result<(), AuthorError> {
    entry_mut(project, id)?.side = side;
    Ok(())
}

pub fn add_group(project: &mut Project, group: Group) -> Result<(), AuthorError> {
    if project.groups.iter().any(|g| g.id == group.id) {
        return Err(AuthorError::GroupExists(group.id));
    }
    project.groups.push(group);
    Ok(())
}

/// Removes a group; its entries become unconditional. Returns how many there were.
pub fn remove_group(project: &mut Project, id: &str) -> Result<usize, AuthorError> {
    let before = project.groups.len();
    project.groups.retain(|g| g.id != id);
    if project.groups.len() == before {
        return Err(AuthorError::NoGroup(id.to_owned()));
    }
    let mut freed = 0;
    for entry in &mut project.content {
        if entry.group.as_deref() == Some(id) {
            entry.group = None;
            freed += 1;
        }
    }
    Ok(freed)
}

/// Puts an entry into a group, or out of any with `None`.
pub fn set_group(
    project: &mut Project,
    id: &str,
    group: Option<String>,
) -> Result<(), AuthorError> {
    if let Some(group) = &group
        && !project.groups.iter().any(|g| &g.id == group)
    {
        return Err(AuthorError::NoGroup(group.clone()));
    }
    entry_mut(project, id)?.group = group;
    Ok(())
}

/// What `riven check` found.
#[derive(Debug, Clone, Default)]
pub struct CheckReport {
    /// Structural problems of `riven.json` itself.
    pub issues: Vec<String>,
    /// Dependency, conflict and loader problems from jar metadata.
    pub problems: Vec<riven_resolve::Problem>,
    /// Jars that could not be read.
    pub unreadable: Vec<String>,
}

impl CheckReport {
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty() && self.problems.is_empty()
    }
}

type MetaCache = std::sync::Mutex<std::collections::HashMap<String, JarMeta>>;

/// Jar metadata by sha512: a file's bytes never change, so each is read once per run.
fn metas() -> &'static MetaCache {
    static METAS: std::sync::OnceLock<MetaCache> = std::sync::OnceLock::new();
    METAS.get_or_init(Default::default)
}

/// Local files can change under the hash they were added with, so only downloads are kept.
fn cache_key(entry: &Entry) -> Option<&String> {
    match entry.source {
        EntrySource::Local { .. } => None,
        _ => entry.file.hashes.sha512.as_ref(),
    }
}

fn remembered(entry: &Entry) -> Option<JarMeta> {
    let key = cache_key(entry)?;
    metas().lock().ok()?.get(key).cloned()
}

fn remember(entry: &Entry, meta: &JarMeta) {
    if let (Some(key), Ok(mut metas)) = (cache_key(entry), metas().lock()) {
        metas.insert(key.clone(), meta.clone());
    }
}

/// Validates `riven.json` and checks every mod's metadata against the others and the game.
pub async fn check(ws: &Workspace) -> Result<CheckReport, AuthorError> {
    let project = &ws.project;
    let issues = project.validate().iter().map(ToString::to_string).collect();
    let jars = ws.jars()?;
    let mods: Vec<Entry> = project
        .content
        .iter()
        .filter(|e| e.kind == Kind::Mod)
        .cloned()
        .collect();
    let jars = &jars;
    let read: Vec<(Entry, Result<JarMeta, String>)> = stream::iter(mods)
        .map(|entry| async move {
            if let Some(meta) = remembered(&entry) {
                return (entry, Ok(meta));
            }
            let meta = match jars.jar(&entry).await {
                Ok(bytes) => JarMeta::read(&bytes).map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            if let Ok(meta) = &meta {
                remember(&entry, meta);
            }
            (entry, meta)
        })
        .buffer_unordered(PARALLEL_DOWNLOADS)
        .collect()
        .await;

    let mut unreadable = Vec::new();
    let mut metas = Vec::new();
    for (entry, meta) in read {
        match meta {
            Ok(meta) => metas.push((entry, meta)),
            Err(e) => unreadable.push(format!("cannot read `{}`: {e}", entry.id)),
        }
    }
    let installed: Vec<Installed> = metas
        .iter()
        .map(|(entry, meta)| Installed {
            entry: &entry.id,
            side: entry.side,
            meta,
        })
        .collect();
    let env = Env {
        minecraft: ModVersion::parse(&project.minecraft),
        loader: project.loader.kind,
        loader_version: ModVersion::parse(&project.loader.version),
        java: project.java.major,
    };
    Ok(CheckReport {
        issues,
        problems: riven_resolve::check(&env, &installed),
        unreadable,
    })
}

/// The next version: `major`, `minor`, `patch` of a MAJOR.MINOR.PATCH one, or `to` itself.
pub fn bump(old: &str, to: &str) -> Result<String, AuthorError> {
    match to {
        "major" | "minor" | "patch" => {
            let core = old.split(['-', '+']).next().unwrap_or(old);
            let parts: Vec<u64> = core
                .split('.')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()
                .filter(|p: &Vec<u64>| p.len() == 3)
                .ok_or_else(|| AuthorError::NotSemver(old.to_owned()))?;
            let [major, minor, patch] = [parts[0], parts[1], parts[2]];
            Ok(match to {
                "major" => format!("{}.0.0", major + 1),
                "minor" => format!("{major}.{}.0", minor + 1),
                _ => format!("{major}.{minor}.{}", patch + 1),
            })
        }
        explicit if explicit.trim().is_empty() => Err(AuthorError::EmptyVersion),
        explicit => Ok(explicit.trim().to_owned()),
    }
}

/// Replaces a group's name, description and default, keeping its id and members.
pub fn update_group(project: &mut Project, group: Group) -> Result<(), AuthorError> {
    let slot = project
        .groups
        .iter_mut()
        .find(|g| g.id == group.id)
        .ok_or_else(|| AuthorError::NoGroup(group.id.clone()))?;
    *slot = group;
    Ok(())
}

/// Entries whose `requires` names `id`.
pub fn required_by<'a>(project: &'a Project, id: &str) -> Vec<&'a Entry> {
    project
        .content
        .iter()
        .filter(|e| e.requires.iter().any(|r| r == id))
        .collect()
}

/// Every chain from an explicitly added entry down to `id`, root first.
pub fn why_paths(project: &Project, id: &str) -> Result<Vec<Vec<String>>, AuthorError> {
    if project.entry(id).is_none() {
        return Err(AuthorError::NoEntry(id.to_owned()));
    }
    let mut paths: Vec<Vec<String>> = Vec::new();
    let mut stack: Vec<Vec<String>> = vec![vec![id.to_owned()]];
    while let Some(path) = stack.pop() {
        let head = &path[0];
        let is_root = project
            .entry(head)
            .is_some_and(|e| e.reason == riven_format::Reason::Explicit);
        if is_root && path.len() > 1 {
            paths.push(path.clone());
        }
        for parent in required_by(project, head) {
            if path.contains(&parent.id) {
                continue;
            }
            let mut longer = vec![parent.id.clone()];
            longer.extend(path.iter().cloned());
            stack.push(longer);
        }
    }
    paths.sort();
    Ok(paths)
}
