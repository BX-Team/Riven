use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::bail;
use console::style;
use riven_build::ship::{self, ExportKind};
use riven_format::InstallSide;
use serde_json::json;

use super::output::Output;
use super::project::Workspace;
use super::{ExportFormat, TrustCommand};

pub fn keygen(out: &Output) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let (key, path, created) = ship::keygen(&ws.project.id)?;
    let public = key.public().to_string();
    out.emit(
        json!({ "key": public, "path": path, "created": created }),
        || {
            if created {
                out.success(&format!("Created the signing key {}", path.display()));
                out.note("back it up: installers refuse updates signed by another key");
            } else {
                out.note(&format!("{} already exists", path.display()));
            }
            println!("{public}");
        },
    );
    Ok(ExitCode::SUCCESS)
}

pub fn trust(out: &Output, command: TrustCommand) -> anyhow::Result<ExitCode> {
    let path = riven_sync::trust::default_path()?;
    let mut trusted = riven_sync::trust::load(&path)?;
    match command {
        TrustCommand::List => out.emit(json!(trusted.keys), || {
            if trusted.keys.is_empty() {
                out.note("no pinned keys yet");
            }
            for (url, key) in &trusted.keys {
                println!("{url}  {}", style(key).dim());
            }
        }),
        TrustCommand::Reset { url } => {
            if trusted.keys.remove(&url).is_none() {
                bail!("no key is pinned for {url}");
            }
            riven_sync::trust::save(&path, &trusted)?;
            out.emit(json!({ "reset": url }), || {
                out.success(&format!(
                    "Forgot the key of {url}; the next install pins the one it serves"
                ))
            });
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn build(out: &Output, channel: &str, dist: &Path, unsigned: bool) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let dist = ws.dir.join(dist);
    let key = ship::signing_key(&ws.project.id, unsigned)?;
    let spinner = out.spinner("Building the release");
    let built = ship::build(&ws, &dist, channel, key.as_ref());
    spinner.finish_and_clear();
    let (built, pointer) = built?;

    let release = &built.release;
    let public = key.as_ref().map(|k| k.public().to_string());
    out.emit(
        json!({
            "version": release.version,
            "channel": channel,
            "files": release.files.len(),
            "blobs": built.blobs.len(),
            "dist": dist,
            "key": public,
        }),
        || {
            if key.is_none() {
                out.warn("the release is unsigned; installers cannot tell it from a forgery");
            }
            out.success(&format!(
                "Built {} {}: {} files ({} blobs), channel `{channel}` → {}",
                release.id,
                release.version,
                release.files.len(),
                built.blobs.len(),
                pointer.version
            ));
            eprintln!(
                "{}",
                style(format!(
                    "push it to GitHub Pages with `riven deploy`, or upload {} and install with: riven install <url>/channels/{channel}.json{}",
                    dist.display(),
                    public
                        .as_ref()
                        .map(|k| format!("#key={k}"))
                        .unwrap_or_default()
                ))
                .dim()
            );
        },
    );
    Ok(ExitCode::SUCCESS)
}

pub fn publish(
    out: &Output,
    channel: &str,
    version: &str,
    dist: &Path,
) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let key = ship::load_key(&ws.project.id)?;
    let dist = ws.dir.join(dist);
    let pointer = riven_build::release::publish(&dist, channel, version, key.as_ref())?;
    out.emit(json!(pointer), || {
        if key.is_none() {
            out.warn("no signing key: the channel pointer is unsigned");
        }
        out.success(&format!("Channel `{channel}` → {}", pointer.version));
    });
    Ok(ExitCode::SUCCESS)
}

pub fn deploy(
    out: &Output,
    dist: &Path,
    branch: &str,
    remote: &str,
    push: bool,
) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let dist = ws.dir.join(dist);
    let spinner = out.spinner(format!("Deploying {} to `{branch}`", dist.display()));
    let shipped = ship::deploy(&ws, &dist, branch, remote, push);
    spinner.finish_and_clear();
    let ship::Shipped { deployed, link } = shipped?;
    out.emit(
        json!({
            "commit": deployed.commit,
            "created": deployed.created,
            "pushed": deployed.pushed,
            "restored": deployed.restored,
            "pages": deployed.pages,
            "link": link,
        }),
        || {
            match (&deployed.commit, deployed.pushed) {
                (Some(commit), true) => out.success(&format!(
                    "Deployed to `{branch}` ({}) and pushed to {remote}",
                    &commit[..7]
                )),
                (Some(commit), false) => {
                    out.success(&format!("Committed to `{branch}` ({})", &commit[..7]))
                }
                (None, true) => out.success(&format!("Pushed `{branch}` to {remote}")),
                (None, false) => out.success(&format!("`{branch}` is already up to date")),
            }
            if deployed.restored > 0 {
                out.note(&format!(
                    "fetched {} published files back into {}",
                    deployed.restored,
                    dist.display()
                ));
            }
            if deployed.created {
                out.note(&format!(
                    "enable GitHub Pages once: Settings → Pages → Deploy from a branch → `{branch}` / (root)"
                ));
            }
            if let Some(link) = &link {
                eprintln!("{}", style(format!("install with: riven install {link}")).dim());
            }
        },
    );
    Ok(ExitCode::SUCCESS)
}

pub async fn export(
    out: &Output,
    format: ExportFormat,
    side: Option<InstallSide>,
    path: Option<PathBuf>,
) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let kind = match format {
        ExportFormat::Mrpack => ExportKind::Mrpack,
        ExportFormat::Prism => ExportKind::Prism,
        ExportFormat::Riven => ExportKind::Riven,
    };
    let spinner = out.spinner("Exporting the pack");
    let report = ship::export(&ws, kind, side, path).await;
    spinner.finish_and_clear();
    let ship::ExportReport {
        path,
        exported: report,
        failures,
    } = report?;

    out.emit(
        json!({
            "path": path,
            "linked": report.linked,
            "embedded": report.embedded,
            "skipped": report.skipped.iter().map(|(id, why)| json!({ "id": id, "reason": why })).collect::<Vec<_>>(),
        }),
        || {
            for failure in &failures {
                out.warn(failure);
            }
            for (id, why) in &report.skipped {
                out.warn(&format!("`{id}` left out: {why}"));
            }
            out.success(&format!(
                "Exported {} ({} linked, {} embedded)",
                path.display(),
                report.linked,
                report.embedded
            ));
        },
    );
    Ok(ExitCode::SUCCESS)
}
