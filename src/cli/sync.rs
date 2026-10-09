use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use console::style;
use riven_format::Release;
use riven_sync::install::{self, Event, Planned};
use riven_sync::remote::{self, Fetched};
use riven_sync::update::{self, Check, DEFAULT_CHANNEL, Request};
use riven_sync::{Store, trust};
use serde_json::json;

use super::InstallArgs;
use super::output::Output;

fn game_dir(dir: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match dir {
        Some(dir) => Ok(dir),
        None => std::env::current_dir().context("cannot read the current directory"),
    }
}

fn store() -> anyhow::Result<Store> {
    Store::default_location().context("cannot locate the data directory")
}

fn print_plan(planned: &Planned, release: &Release) {
    let summary = format!(
        "{} {}: {} to install, {} to remove, {} up to date",
        release.name,
        release.version,
        planned.install.len(),
        planned.remove.len(),
        planned.unchanged.len()
    );
    eprintln!("{}", style(summary).bold());
    for (path, _) in &planned.kept {
        eprintln!("  {} {path} (changed locally, kept)", style("=").dim());
    }
    for path in &planned.remove {
        eprintln!("  {} {path}", style("-").red());
    }
}

pub async fn install(out: &Output, args: InstallArgs) -> anyhow::Result<ExitCode> {
    let dir = game_dir(args.dir)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let store = store()?;
    let http = riven_sources::client();
    let request = Request {
        source: args.source,
        channel: args.channel,
        side: args.side.map(Into::into),
        groups: args.groups,
        replaces: false,
    };

    let spinner = out.spinner("Checking the pack");
    let pending = match update::check(&store, &http, &dir, &request).await? {
        Check::UpToDate { version } => {
            spinner.finish_and_clear();
            out.emit(json!({ "updated": false, "version": version }), || {
                out.success(&format!("Up to date ({version})"))
            });
            return Ok(ExitCode::SUCCESS);
        }
        Check::Ready(pending) => pending,
    };
    spinner.finish_and_clear();

    let planned = &pending.planned;
    if !planned.is_noop() {
        if !args.headless {
            print_plan(planned, &pending.release);
        }
        if !(args.yes || args.headless) && !out.confirm("Apply?", true) {
            bail!("cancelled");
        }
    }
    let installed = planned.install.len();
    let removed = planned.remove.len();
    let kept: Vec<String> = planned.kept.iter().map(|(p, _)| p.to_string()).collect();
    let name = pending.release.name.clone();
    let bar = out.spinner("Downloading");
    let progress = |event: Event| match event {
        Event::Downloading { done, total } => {
            bar.set_message(format!("Downloading {done}/{total}"))
        }
        Event::Installing { done, total } => bar.set_message(format!("Installing {done}/{total}")),
    };
    let state = pending.apply(&store, &http, &dir, &progress).await?;
    bar.finish_and_clear();

    let groups_on: Vec<&String> = state
        .groups
        .iter()
        .filter(|(_, on)| **on)
        .map(|(g, _)| g)
        .collect();
    out.emit(
        json!({
            "updated": true,
            "version": state.version,
            "installed": installed,
            "removed": removed,
            "kept": kept,
            "groups": state.groups,
        }),
        || {
            out.success(&format!(
                "{name} {} installed in {} ({installed} files changed, {removed} removed){}",
                state.version,
                dir.display(),
                if groups_on.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; groups: {}",
                        groups_on
                            .iter()
                            .map(|g| g.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            ));
        },
    );
    Ok(ExitCode::SUCCESS)
}

pub async fn status(out: &Output, dir: Option<PathBuf>) -> anyhow::Result<ExitCode> {
    let dir = game_dir(dir)?;
    let Some(state) = install::load_state(&dir)? else {
        bail!("nothing is installed in {}", dir.display());
    };
    let latest = if state.source.starts_with("http") {
        let link = remote::parse_link(&state.source, DEFAULT_CHANNEL)?;
        let mut scratch = trust::load(&trust::default_path()?)?;
        match remote::fetch(
            &riven_sources::client(),
            &link,
            state.etag.as_deref(),
            &mut scratch,
        )
        .await
        {
            Ok(Fetched::NotModified) => Some(state.version.clone()),
            Ok(Fetched::Release(remote)) => Some(remote.release.version),
            Err(e) => {
                out.warn(&format!("cannot check for updates: {e}"));
                None
            }
        }
    } else {
        None
    };
    let groups: BTreeMap<&String, &bool> = state.groups.iter().collect();
    out.emit(
        json!({
            "source": state.source,
            "version": state.version,
            "latest": latest,
            "side": state.side,
            "groups": groups,
            "files": state.files.len(),
            "signed": state.key.is_some(),
        }),
        || {
            println!("{} {}", style("source").dim(), state.source);
            println!("{} {}", style("version").dim(), state.version);
            println!("{} {:?}", style("side").dim(), state.side);
            println!("{} {}", style("files").dim(), state.files.len());
            for (group, on) in &groups {
                println!(
                    "{} {group}: {}",
                    style("group").dim(),
                    if **on { "on" } else { "off" }
                );
            }
            match &latest {
                Some(latest) if *latest != state.version => {
                    out.note(&format!("update available: {latest}; run `riven install`"))
                }
                Some(_) => out.success("Up to date"),
                None => {}
            }
        },
    );
    Ok(ExitCode::SUCCESS)
}
