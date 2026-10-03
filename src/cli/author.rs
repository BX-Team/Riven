use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, anyhow, bail};
use console::style;
use futures_util::{StreamExt, stream};
use riven_format::{
    Entry, FileRules, Group, Java, Kind, Loader, PackPath, Project, Reason, Side,
    Source as EntrySource, SourceKind, UpdatePolicy,
};
use riven_resolve::{
    AddOptions, Downloader, Env, Installed, JarFetcher, JarMeta, ModVersion, Planner, carry_over,
    direct_entry, github_update, rehome,
};
use riven_sources::{Known, Modrinth, Source, Target};
use serde_json::json;

use super::output::{Output, file_name, side_label};
use super::project::{PROJECT_FILE, StoreJars, Workspace, game_meta, write_atomic};
use super::{GroupCommand, SourceArg, parse_loader};

const PARALLEL_DOWNLOADS: usize = 8;

pub async fn init(out: &Output, mc: Option<String>, loader: &str) -> anyhow::Result<ExitCode> {
    let dir = std::env::current_dir().context("cannot read the current directory")?;
    let path = dir.join(PROJECT_FILE);
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    let (kind, loader_version) = parse_loader(loader)?;
    let cache = riven_sources::Cache::new(
        dir.join(".riven").join("cache"),
        std::time::Duration::from_secs(300),
    );
    let game = game_meta(Some(cache));
    let spinner = out.spinner("Fetching version metadata");
    let minecraft = match mc {
        Some(mc) => mc,
        None => game.latest_minecraft().await?,
    };
    let java = game.java_major(&minecraft).await?;
    let loader_version = match loader_version {
        Some(v) => v,
        None => game.loader_version(kind, &minecraft).await?,
    };
    spinner.finish_and_clear();

    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "modpack".into());
    let project = Project {
        id: slugify(&name),
        name,
        version: "0.1.0".into(),
        authors: vec![],
        description: None,
        icon: None,
        minecraft,
        loader: Loader {
            kind,
            version: loader_version,
        },
        java: Java {
            major: java,
            memory: None,
            jvm_args: vec![],
        },
        groups: vec![],
        files: FileRules::default(),
        content: vec![],
    };
    write_atomic(&path, &riven_format::to_string(&project))?;
    ensure_gitignore(&dir)?;

    out.emit(json!({ "project": project }), || {
        out.success(&format!(
            "Created {PROJECT_FILE} for `{}`: Minecraft {}, {} {}, Java {}",
            project.id,
            project.minecraft,
            riven_sources::loader_name(kind),
            project.loader.version,
            project.java.major
        ));
    });
    Ok(ExitCode::SUCCESS)
}

fn slugify(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "modpack".into()
    } else {
        slug
    }
}

pub(super) fn ensure_gitignore(dir: &std::path::Path) -> anyhow::Result<()> {
    let path = dir.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = [".riven/", "dist/"]
        .into_iter()
        .filter(|line| !existing.lines().any(|l| l.trim() == *line))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut contents = existing;
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    for line in missing {
        contents.push_str(line);
        contents.push('\n');
    }
    std::fs::write(&path, contents).with_context(|| format!("cannot write {}", path.display()))
}

/// Turns a slug, id, Modrinth URL or search query into a project id and an optional version.
async fn find_project(
    modrinth: &Modrinth,
    pack: &Project,
    query: &str,
) -> anyhow::Result<(String, Option<String>)> {
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
            _ => bail!("`{query}` is not a Modrinth project or version URL"),
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
        (None, []) => bail!(
            "nothing on Modrinth matches `{query}` for Minecraft {}",
            pack.minecraft
        ),
        (None, hits) => {
            let options: Vec<String> = hits
                .iter()
                .map(|h| format!("  {} — {}", h.slug, h.title))
                .collect();
            bail!(
                "`{query}` is ambiguous; add one by slug:\n{}",
                options.join("\n")
            )
        }
    }
}

/// `riven add` flags besides the query.
pub struct AddArgs {
    pub source: Option<SourceArg>,
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

fn origin(query: &str, source: Option<SourceArg>) -> anyhow::Result<Origin> {
    let web = url::Url::parse(query)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"));
    let host = web.as_ref().and_then(|u| u.host_str()).unwrap_or_default();
    Ok(match source {
        Some(SourceArg::Modrinth) => Origin::Modrinth,
        Some(SourceArg::Github) => github_origin(query)?,
        Some(SourceArg::Url) => match web {
            Some(_) => Origin::Url(query.to_owned()),
            None => bail!("`{query}` is not an http(s) URL"),
        },
        Some(SourceArg::Local) => Origin::Local(query.into()),
        None if host == "github.com" || host == "www.github.com" => github_origin(query)?,
        None if host.ends_with("modrinth.com") => Origin::Modrinth,
        None if web.is_some() => Origin::Url(query.to_owned()),
        None if Path::new(query).is_file() => Origin::Local(query.into()),
        None => Origin::Modrinth,
    })
}

/// Parses `owner/repo` or a GitHub repository, release or release asset URL.
fn github_origin(query: &str) -> anyhow::Result<Origin> {
    let path = match url::Url::parse(query) {
        Ok(url) => url.path().to_owned(),
        Err(_) => query.to_owned(),
    };
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (owner, repo, rest) = match segments.as_slice() {
        [owner, repo, rest @ ..] => (*owner, repo.trim_end_matches(".git"), rest),
        _ => bail!("`{query}` is not a GitHub repository (owner/name or a github.com URL)"),
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
        _ => bail!("`{query}` is not a GitHub repository, release or release asset URL"),
    };
    Ok(Origin::GitHub {
        repo: format!("{owner}/{repo}"),
        tag,
        asset,
    })
}

/// The decoded last path segment of `url`, used as the installed file name.
fn url_file_name(url: &str) -> anyhow::Result<String> {
    let parsed = url::Url::parse(url).with_context(|| format!("`{url}` is not a URL"))?;
    parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .map(|s| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8_lossy()
                .into_owned()
        })
        .with_context(|| format!("`{url}` does not end in a file name"))
}

/// The pack path of a local file; files outside the repository are copied into `local/`.
fn local_path(ws: &Workspace, file: &Path) -> anyhow::Result<PackPath> {
    let file =
        std::fs::canonicalize(file).with_context(|| format!("cannot find {}", file.display()))?;
    if !file.is_file() {
        bail!("{} is not a file", file.display());
    }
    let root = std::fs::canonicalize(&ws.dir)
        .with_context(|| format!("cannot read {}", ws.dir.display()))?;
    let relative = match file.strip_prefix(&root) {
        Ok(relative) => relative.to_owned(),
        Err(_) => {
            let name = file.file_name().context("the file has no name")?;
            let target = ws.dir.join("local").join(name);
            if target.exists() {
                if std::fs::read(&target)? != std::fs::read(&file)? {
                    bail!("{} already exists with other contents", target.display());
                }
            } else {
                std::fs::create_dir_all(ws.dir.join("local"))?;
                std::fs::copy(&file, &target)
                    .with_context(|| format!("cannot copy into {}", target.display()))?;
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
        bail!(
            "keep local files under local/ or overrides/ (got {})",
            relative.display()
        );
    }
    Ok(PackPath::new(parts.join("/"))?)
}

/// Builds the entry for a GitHub, URL or local origin.
async fn direct_add(
    ws: &Workspace,
    jars: &StoreJars,
    origin: Origin,
    asset: Option<String>,
) -> anyhow::Result<Entry> {
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
            let file = jars
                .download(&url)
                .await
                .map_err(|e| anyhow!("cannot download {url}: {e}"))?;
            let source = EntrySource::Url { url: url.clone() };
            direct_entry(pack, source, &name, &file, Some(url))?
        }
        Origin::Local(path) => {
            let path = local_path(ws, &path)?;
            let bytes = std::fs::read(ws.dir.join(path.as_str()))
                .with_context(|| format!("cannot read {path}"))?;
            let file = jars.keep(bytes, path.as_str()).map_err(|e| anyhow!(e))?;
            let name = path.file_name().to_owned();
            direct_entry(pack, EntrySource::Local { path }, &name, &file, None)?
        }
        Origin::Modrinth => unreachable!("Modrinth projects go through the planner"),
    })
}

pub async fn add(out: &Output, query: &str, args: AddArgs) -> anyhow::Result<ExitCode> {
    let mut ws = Workspace::find()?;
    if let Some(group) = &args.group
        && !ws.project.groups.iter().any(|g| &g.id == group)
    {
        bail!("no group `{group}`; create it with `riven group add {group} --name …`");
    }
    let origin = origin(query, args.source)?;
    if args.asset.is_some() && !matches!(origin, Origin::GitHub { .. }) {
        bail!("--asset only applies to GitHub sources");
    }
    let modrinth = ws.modrinth();
    let jars = ws.jars()?;
    let spinner = out.spinner(format!("Resolving `{query}`"));
    let plan = if let Origin::Modrinth = origin {
        let (project_id, version) = find_project(&modrinth, &ws.project, query).await?;
        let options = AddOptions {
            version,
            side: args.side,
            group: args.group,
            pin: args.pin,
        };
        Planner::new(&modrinth, &jars, &ws.project)
            .add(&project_id, &options)
            .await
    } else {
        let mut entry = direct_add(&ws, &jars, origin, args.asset).await?;
        if let Some(side) = args.side {
            entry.side = side;
        }
        entry.group = args.group;
        if args.pin {
            entry.update = UpdatePolicy::Pinned;
        }
        Planner::new(&modrinth, &jars, &ws.project)
            .add_entry(entry)
            .await
    };
    spinner.finish_and_clear();
    let plan = plan?;

    out.plan(&plan);
    plan.apply(&mut ws.project);
    ws.save()?;
    out.success(&format!(
        "Added {} {}",
        plan.add.len(),
        if plan.add.len() == 1 {
            "entry"
        } else {
            "entries"
        }
    ));
    Ok(ExitCode::SUCCESS)
}

pub async fn remove(out: &Output, id: &str, keep_deps: bool) -> anyhow::Result<ExitCode> {
    let mut ws = Workspace::find()?;
    let modrinth = ws.modrinth();
    let jars = ws.jars()?;
    let plan = Planner::new(&modrinth, &jars, &ws.project).remove(id, keep_deps)?;
    out.plan(&plan);
    plan.apply(&mut ws.project);
    ws.save()?;
    out.success(&format!("Removed {}", plan.remove.len()));
    Ok(ExitCode::SUCCESS)
}

/// Updates the planner cannot find itself: GitHub releases, changed local files, `--url`.
async fn direct_updates(
    ws: &Workspace,
    jars: &StoreJars,
    ids: &[String],
    url: Option<&str>,
) -> anyhow::Result<(Vec<(Entry, Entry)>, Vec<String>)> {
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
                bail!(
                    "`{}` is not a URL entry; --url only repoints URL sources",
                    entry.id
                );
            }
            let file = jars
                .download(url)
                .await
                .map_err(|e| anyhow!("cannot download {url}: {e}"))?;
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
                let bytes = std::fs::read(ws.dir.join(path.as_str()))
                    .with_context(|| format!("cannot read {path} for `{}`", entry.id))?;
                let file = jars.keep(bytes, path.as_str()).map_err(|e| anyhow!(e))?;
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

fn source_name(kind: SourceKind) -> &'static str {
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
) -> anyhow::Result<Vec<(Entry, Entry)>> {
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

/// Plans updates of every kind of source.
async fn plan_updates(
    ws: &Workspace,
    ids: &[String],
    url: Option<&str>,
) -> anyhow::Result<riven_resolve::Plan> {
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

pub async fn update(
    out: &Output,
    ids: &[String],
    dry_run: bool,
    url: Option<&str>,
) -> anyhow::Result<ExitCode> {
    if url.is_some() && ids.len() != 1 {
        bail!("--url repoints one entry: riven update <id> --url <url>");
    }
    let mut ws = Workspace::find()?;
    let spinner = out.spinner("Checking for updates");
    let plan = plan_updates(&ws, ids, url).await;
    spinner.finish_and_clear();
    let plan = plan?;

    out.plan(&plan);
    if plan.is_empty() {
        out.success("Everything is up to date");
    } else if dry_run {
        out.note("dry run: riven.json left unchanged");
    } else {
        plan.apply(&mut ws.project);
        ws.save()?;
        out.success(&format!("Updated {}", plan.update.len()));
    }
    Ok(ExitCode::SUCCESS)
}

fn entry_mut<'a>(
    project: &'a mut Project,
    id: &str,
) -> anyhow::Result<&'a mut riven_format::Entry> {
    project
        .content
        .iter_mut()
        .find(|e| e.id == id)
        .with_context(|| format!("no entry `{id}` in the pack"))
}

pub fn set_pinned(out: &Output, id: &str, pinned: bool) -> anyhow::Result<ExitCode> {
    let mut ws = Workspace::find()?;
    let entry = entry_mut(&mut ws.project, id)?;
    entry.update = if pinned {
        UpdatePolicy::Pinned
    } else {
        UpdatePolicy::Follow
    };
    ws.save()?;
    out.emit(json!({ "id": id, "pinned": pinned }), || {
        out.success(&format!(
            "`{id}` is {}",
            if pinned { "pinned" } else { "unpinned" }
        ));
    });
    Ok(ExitCode::SUCCESS)
}

pub fn set_side(out: &Output, id: &str, side: Side) -> anyhow::Result<ExitCode> {
    let mut ws = Workspace::find()?;
    entry_mut(&mut ws.project, id)?.side = side;
    ws.save()?;
    out.emit(json!({ "id": id, "side": side }), || {
        out.success(&format!("`{id}` now installs on {}", side_name(side)));
    });
    Ok(ExitCode::SUCCESS)
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Client => "the client",
        Side::Server => "the server",
        Side::Both => "both sides",
    }
}

pub fn group(out: &Output, command: GroupCommand) -> anyhow::Result<ExitCode> {
    let mut ws = Workspace::find()?;
    let project = &mut ws.project;
    let message = match command {
        GroupCommand::Add {
            id,
            name,
            description,
            default,
        } => {
            if project.groups.iter().any(|g| g.id == id) {
                bail!("group `{id}` already exists");
            }
            project.groups.push(Group {
                id: id.clone(),
                name,
                description,
                default,
            });
            format!("Added group `{id}`")
        }
        GroupCommand::Rm { id } => {
            let before = project.groups.len();
            project.groups.retain(|g| g.id != id);
            if project.groups.len() == before {
                bail!("no group `{id}`");
            }
            let mut freed = 0;
            for entry in &mut project.content {
                if entry.group.as_deref() == Some(id.as_str()) {
                    entry.group = None;
                    freed += 1;
                }
            }
            format!("Removed group `{id}`; {freed} entries are now unconditional")
        }
        GroupCommand::Set { entry, group } => {
            let group = (group != "none").then_some(group);
            if let Some(group) = &group
                && !project.groups.iter().any(|g| &g.id == group)
            {
                bail!("no group `{group}`");
            }
            entry_mut(project, &entry)?.group = group.clone();
            match group {
                Some(group) => format!("`{entry}` is now in group `{group}`"),
                None => format!("`{entry}` is no longer in a group"),
            }
        }
    };
    ws.save()?;
    out.emit(json!({ "groups": ws.project.groups }), || {
        out.success(&message)
    });
    Ok(ExitCode::SUCCESS)
}

pub async fn list(out: &Output, tree: bool, outdated: bool) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let project = &ws.project;
    if outdated {
        let spinner = out.spinner("Checking for updates");
        let plan = plan_updates(&ws, &[], None).await;
        spinner.finish_and_clear();
        let plan = plan?;
        let rows: Vec<_> = plan
            .update
            .iter()
            .map(|(old, new)| json!({ "id": new.id, "current": file_name(old), "latest": file_name(new) }))
            .collect();
        out.emit(json!(rows), || {
            if plan.update.is_empty() {
                out.success("Everything is up to date");
            }
            for (old, new) in &plan.update {
                println!(
                    "{} {} → {}",
                    style(&new.id).bold(),
                    style(file_name(old)).dim(),
                    file_name(new)
                );
            }
        });
        return Ok(ExitCode::SUCCESS);
    }

    let mut entries: Vec<&riven_format::Entry> = project.content.iter().collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    if tree {
        out.emit(json!(tree_json(project)), || print_tree(project));
        return Ok(ExitCode::SUCCESS);
    }
    out.emit(json!(entries), || {
        let width = entries.iter().map(|e| e.id.len()).max().unwrap_or(0);
        for entry in &entries {
            let mut flags = Vec::new();
            if entry.update == UpdatePolicy::Pinned {
                flags.push("pinned".to_owned());
            }
            if let Some(group) = &entry.group {
                flags.push(format!("group:{group}"));
            }
            if entry.reason == Reason::Dependency {
                flags.push("dependency".to_owned());
            }
            let flags = if flags.is_empty() {
                String::new()
            } else {
                style(format!(" ({})", flags.join(", "))).dim().to_string()
            };
            println!(
                "{:width$}  {}{}{}",
                style(&entry.id).bold(),
                file_name(entry),
                side_label(entry.side),
                flags
            );
        }
        eprintln!("{}", style(format!("{} entries", entries.len())).dim());
    });
    Ok(ExitCode::SUCCESS)
}

fn tree_json(project: &Project) -> serde_json::Value {
    fn node(project: &Project, id: &str, seen: &mut HashSet<String>) -> serde_json::Value {
        let requires: Vec<_> = match project.entry(id) {
            Some(entry) if seen.insert(id.to_owned()) => entry
                .requires
                .iter()
                .map(|r| node(project, r, seen))
                .collect(),
            _ => vec![],
        };
        json!({ "id": id, "requires": requires })
    }
    let roots: Vec<_> = explicit(project)
        .map(|e| node(project, &e.id, &mut HashSet::new()))
        .collect();
    json!(roots)
}

fn explicit(project: &Project) -> impl Iterator<Item = &riven_format::Entry> {
    let mut roots: Vec<_> = project
        .content
        .iter()
        .filter(|e| e.reason == Reason::Explicit)
        .collect();
    roots.sort_by(|a, b| a.id.cmp(&b.id));
    roots.into_iter()
}

fn print_tree(project: &Project) {
    fn walk(project: &Project, id: &str, depth: usize, stack: &mut Vec<String>) {
        let indent = "  ".repeat(depth);
        let Some(entry) = project.entry(id) else {
            println!("{indent}{} {}", id, style("(missing)").red());
            return;
        };
        if stack.iter().any(|s| s == id) {
            println!("{indent}{} {}", id, style("(cycle)").yellow());
            return;
        }
        println!(
            "{indent}{} {}{}",
            if depth == 0 {
                style(id).bold()
            } else {
                style(id)
            },
            style(file_name(entry)).dim(),
            side_label(entry.side)
        );
        stack.push(id.to_owned());
        let mut requires = entry.requires.clone();
        requires.sort();
        for r in &requires {
            walk(project, r, depth + 1, stack);
        }
        stack.pop();
    }
    for root in explicit(project) {
        walk(project, &root.id, 0, &mut Vec::new());
    }
}

pub fn why(out: &Output, id: &str) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let project = &ws.project;
    let entry = project
        .entry(id)
        .with_context(|| format!("no entry `{id}` in the pack"))?;

    let mut paths: Vec<Vec<String>> = Vec::new();
    let mut stack: Vec<Vec<String>> = vec![vec![id.to_owned()]];
    while let Some(path) = stack.pop() {
        let head = &path[0];
        let parents: Vec<&riven_format::Entry> = project
            .content
            .iter()
            .filter(|e| e.requires.contains(head) && !path.contains(&e.id))
            .collect();
        let is_root = project
            .entry(head)
            .is_some_and(|e| e.reason == Reason::Explicit);
        if is_root && path.len() > 1 {
            paths.push(path.clone());
        }
        for parent in parents {
            let mut longer = vec![parent.id.clone()];
            longer.extend(path.iter().cloned());
            stack.push(longer);
        }
    }
    paths.sort();
    let required_by: Vec<&str> = project
        .content
        .iter()
        .filter(|e| e.requires.iter().any(|r| r == id))
        .map(|e| e.id.as_str())
        .collect();

    out.emit(
        json!({ "id": id, "reason": entry.reason, "required_by": required_by, "paths": paths }),
        || {
            if entry.reason == Reason::Explicit {
                println!("`{id}` was added explicitly");
            }
            if paths.is_empty() && entry.reason == Reason::Dependency {
                println!("`{id}` is an orphaned dependency: nothing requires it");
            }
            for path in &paths {
                println!("{}", path.join(" → "));
            }
        },
    );
    Ok(ExitCode::SUCCESS)
}

pub async fn check(out: &Output) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let project = &ws.project;
    let issues: Vec<String> = project.validate().iter().map(ToString::to_string).collect();

    let jars = ws.jars()?;
    let spinner = out.spinner("Reading jar metadata");
    let mods: Vec<&riven_format::Entry> = project
        .content
        .iter()
        .filter(|e| e.kind == Kind::Mod)
        .collect();
    let read: Vec<(&riven_format::Entry, Result<JarMeta, String>)> = stream::iter(mods)
        .map(|entry| {
            let jars = &jars;
            async move {
                let meta = match jars.jar(entry).await {
                    Ok(bytes) => JarMeta::read(&bytes).map_err(|e| e.to_string()),
                    Err(e) => Err(e),
                };
                (entry, meta)
            }
        })
        .buffer_unordered(PARALLEL_DOWNLOADS)
        .collect()
        .await;
    spinner.finish_and_clear();

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
    let problems: Vec<String> = riven_resolve::check(&env, &installed)
        .iter()
        .map(ToString::to_string)
        .collect();

    let clean = issues.is_empty() && problems.is_empty() && unreadable.is_empty();
    out.emit(
        json!({ "ok": clean, "issues": issues, "problems": problems, "unreadable": unreadable }),
        || {
            for line in issues.iter().chain(&problems) {
                eprintln!("{} {line}", style("error:").red().bold());
            }
            for line in &unreadable {
                out.warn(line);
            }
            if clean {
                out.success(&format!("{} entries, no problems", project.content.len()));
            }
        },
    );
    Ok(if issues.is_empty() && problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub fn bump(out: &Output, to: &str) -> anyhow::Result<ExitCode> {
    let mut ws = Workspace::find()?;
    let old = ws.project.version.clone();
    let new = match to {
        "major" | "minor" | "patch" => {
            let core = old.split(['-', '+']).next().unwrap_or(&old);
            let parts: Vec<u64> = core
                .split('.')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()
                .filter(|p: &Vec<u64>| p.len() == 3)
                .with_context(|| {
                    format!("version `{old}` is not MAJOR.MINOR.PATCH; pass an explicit version")
                })?;
            let [major, minor, patch] = [parts[0], parts[1], parts[2]];
            match to {
                "major" => format!("{}.0.0", major + 1),
                "minor" => format!("{major}.{}.0", minor + 1),
                _ => format!("{major}.{minor}.{}", patch + 1),
            }
        }
        explicit if explicit.trim().is_empty() => bail!("empty version"),
        explicit => explicit.trim().to_owned(),
    };
    ws.project.version = new.clone();
    ws.save()?;
    out.emit(json!({ "from": old, "to": new }), || {
        out.success(&format!("{old} → {new}"));
    });
    Ok(ExitCode::SUCCESS)
}
