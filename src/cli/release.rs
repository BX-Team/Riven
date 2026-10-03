use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, bail};
use console::style;
use futures_util::{StreamExt, stream};
use riven_build::export::{Format, needs_bytes, selected};
use riven_format::{InstallSide, KeyPair};
use riven_resolve::JarFetcher;
use serde_json::json;

use super::output::Output;
use super::project::Workspace;
use super::{ExportFormat, TrustCommand};

const PARALLEL_DOWNLOADS: usize = 8;

fn key_path(pack: &str) -> anyhow::Result<PathBuf> {
    let dir = riven_sync::config_dir().context("cannot locate the config directory")?;
    Ok(dir.join("keys").join(format!("{pack}.ed25519")))
}

fn load_key(pack: &str) -> anyhow::Result<Option<KeyPair>> {
    let path = key_path(pack)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(
            KeyPair::from_secret(&text).with_context(|| path.display().to_string())?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn write_secret(path: &Path, text: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().expect("key path has a parent"))?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(text.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, text)?;
    Ok(())
}

pub fn keygen(out: &Output) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let path = key_path(&ws.project.id)?;
    let (key, created) = match load_key(&ws.project.id)? {
        Some(key) => (key, false),
        None => {
            let key = KeyPair::generate()?;
            write_secret(&path, &key.to_secret())
                .with_context(|| format!("cannot write {}", path.display()))?;
            (key, true)
        }
    };
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

pub async fn build(
    out: &Output,
    channel: &str,
    dist: &Path,
    unsigned: bool,
) -> anyhow::Result<ExitCode> {
    let ws = Workspace::find()?;
    let dist = ws.dir.join(dist);
    let key = match (load_key(&ws.project.id)?, unsigned) {
        (_, true) => None,
        (Some(key), false) => Some(key),
        (None, false) => bail!(
            "no signing key for `{}`; run `riven keygen`, or build with --unsigned",
            ws.project.id
        ),
    };
    let spinner = out.spinner("Building the release");
    let built = riven_build::release::build_release(&ws.project, &ws.dir)?;
    riven_build::release::write_release(&dist, &built, key.as_ref())?;
    let pointer = riven_build::release::publish(&dist, channel, &ws.project.version, key.as_ref())?;
    spinner.finish_and_clear();

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
    let key = load_key(&ws.project.id)?;
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
    let message = format!("deploy {} {}", ws.project.id, ws.project.version);
    let options = riven_build::deploy::Deploy {
        dist: &dist,
        branch,
        remote,
        message: &message,
        push,
    };
    let spinner = out.spinner(format!("Deploying {} to `{branch}`", dist.display()));
    let deployed = riven_build::deploy::deploy(&ws.dir, &options);
    spinner.finish_and_clear();
    let deployed = deployed?;
    let key = load_key(&ws.project.id)?.map(|k| k.public().to_string());
    let link = deployed.short_link.as_ref().map(|base| match &key {
        Some(key) => format!("{base}#key={key}"),
        None => base.clone(),
    });
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
    let format = match format {
        ExportFormat::Mrpack => Format::Mrpack,
        ExportFormat::Prism => Format::Prism,
    };
    let suffix = match format {
        Format::Mrpack => "",
        Format::Prism => "-prism",
    };
    super::author::ensure_gitignore(&ws.dir)?;
    let path = path.unwrap_or_else(|| {
        ws.dir.join("exports").join(format!(
            "{}-{}{suffix}.{}",
            ws.project.id,
            ws.project.version,
            format.extension()
        ))
    });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }

    let jars = ws.jars()?;
    let wanted: Vec<_> = selected(format, &ws.project, side)
        .into_iter()
        .filter(|e| needs_bytes(format, e))
        .collect();
    let spinner = out.spinner(format!("Fetching {} files to embed", wanted.len()));
    let failures: Vec<String> = stream::iter(wanted)
        .map(|entry| {
            let jars = &jars;
            async move {
                jars.jar(entry)
                    .await
                    .err()
                    .map(|e| format!("`{}`: {e}", entry.id))
            }
        })
        .buffer_unordered(PARALLEL_DOWNLOADS)
        .filter_map(|e| async move { e })
        .collect()
        .await;
    spinner.set_message("Writing the archive");
    let bytes = |entry: &riven_format::Entry| -> Result<Vec<u8>, String> {
        let file = jars
            .local_path(entry)
            .ok_or_else(|| "not downloaded".to_owned())?;
        std::fs::read(&file).map_err(|e| e.to_string())
    };
    let report = riven_build::export::export(format, &ws.project, &ws.dir, side, &bytes, &path)?;
    spinner.finish_and_clear();

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
