use std::collections::HashSet;
use std::process::ExitCode;

use anyhow::Context;
use console::style;
use riven_build::author::{self as core, AddRequest};
use riven_format::{Group, Project, Reason, Side, SourceKind, UpdatePolicy};
use serde_json::json;

use super::output::{Output, file_name, side_label};
use super::project::{PROJECT_FILE, Workspace};
use super::{GroupCommand, SourceArg, parse_loader};

pub async fn init(out: &Output, mc: Option<String>, loader: &str) -> anyhow::Result<ExitCode> {
    let dir = std::env::current_dir().context("cannot read the current directory")?;
    let (kind, loader_version) = parse_loader(loader)?;
    let spinner = out.spinner("Fetching version metadata");
    let ws = riven_build::workspace::init(&dir, mc, kind, loader_version).await;
    spinner.finish_and_clear();
    let project = ws?.project;
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

/// `riven add` flags besides the query.
pub struct AddArgs {
    pub source: Option<SourceArg>,
    pub asset: Option<String>,
    pub side: Option<Side>,
    pub group: Option<String>,
    pub pin: bool,
}

pub async fn add(out: &Output, query: &str, args: AddArgs) -> anyhow::Result<ExitCode> {
    let mut ws = Workspace::find()?;
    let request = AddRequest {
        query: query.to_owned(),
        source: args.source.map(|s| match s {
            SourceArg::Modrinth => SourceKind::Modrinth,
            SourceArg::Github => SourceKind::GitHub,
            SourceArg::Url => SourceKind::Url,
            SourceArg::Local => SourceKind::Local,
        }),
        asset: args.asset,
        side: args.side,
        group: args.group,
        pin: args.pin,
    };
    let spinner = out.spinner(format!("Resolving `{query}`"));
    let plan = core::plan_add(&ws, &request).await;
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
    let plan = core::plan_remove(&ws, id, keep_deps)?;
    out.plan(&plan);
    plan.apply(&mut ws.project);
    ws.save()?;
    out.success(&format!("Removed {}", plan.remove.len()));
    Ok(ExitCode::SUCCESS)
}

async fn plan_updates(
    ws: &Workspace,
    ids: &[String],
    url: Option<&str>,
) -> anyhow::Result<riven_resolve::Plan> {
    Ok(core::plan_updates(ws, ids, url).await?)
}

pub async fn update(
    out: &Output,
    ids: &[String],
    dry_run: bool,
    url: Option<&str>,
) -> anyhow::Result<ExitCode> {
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

pub fn set_pinned(out: &Output, id: &str, pinned: bool) -> anyhow::Result<ExitCode> {
    let mut ws = Workspace::find()?;
    core::set_pinned(&mut ws.project, id, pinned)?;
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
    core::set_side(&mut ws.project, id, side)?;
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
            core::add_group(
                project,
                Group {
                    id: id.clone(),
                    name,
                    description,
                    default,
                },
            )?;
            format!("Added group `{id}`")
        }
        GroupCommand::Rm { id } => {
            let freed = core::remove_group(project, &id)?;
            format!("Removed group `{id}`; {freed} entries are now unconditional")
        }
        GroupCommand::Set { entry, group } => {
            let group = (group != "none").then_some(group);
            core::set_group(project, &entry, group.clone())?;
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
    let paths = core::why_paths(project, id)?;
    let required_by: Vec<&str> = core::required_by(project, id)
        .into_iter()
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
    let spinner = out.spinner("Reading jar metadata");
    let report = core::check(&ws).await;
    spinner.finish_and_clear();
    let report = report?;
    let problems: Vec<String> = report.problems.iter().map(ToString::to_string).collect();
    let clean = report.is_clean() && report.unreadable.is_empty();
    out.emit(
        json!({ "ok": clean, "issues": report.issues, "problems": problems, "unreadable": report.unreadable }),
        || {
            for line in report.issues.iter().chain(&problems) {
                eprintln!("{} {line}", style("error:").red().bold());
            }
            for line in &report.unreadable {
                out.warn(line);
            }
            if clean {
                out.success(&format!("{} entries, no problems", ws.project.content.len()));
            }
        },
    );
    Ok(if report.is_clean() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub fn bump(out: &Output, to: &str) -> anyhow::Result<ExitCode> {
    let mut ws = Workspace::find()?;
    let old = ws.project.version.clone();
    let new = core::bump(&old, to)?;
    ws.project.version = new.clone();
    ws.save()?;
    out.emit(json!({ "from": old, "to": new }), || {
        out.success(&format!("{old} → {new}"));
    });
    Ok(ExitCode::SUCCESS)
}
