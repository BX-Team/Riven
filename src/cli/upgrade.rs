use std::process::ExitCode;

use riven_launch::update::{self, Edition, Install};
use serde_json::json;

use super::output::Output;

const EDITION: Edition = if cfg!(feature = "gui") {
    Edition::Launcher
} else {
    Edition::Cli
};

pub async fn self_update(out: &Output, check_only: bool) -> anyhow::Result<ExitCode> {
    let spinner = out.spinner("Looking for a new version");
    let release = update::check().await;
    spinner.finish_and_clear();
    let Some(release) = release? else {
        out.emit(
            json!({ "update": false, "version": update::CURRENT }),
            || out.success(&format!("Riven {} is the latest version", update::CURRENT)),
        );
        return Ok(ExitCode::SUCCESS);
    };
    let install = Install::detect();
    if check_only || !install.self_updates() {
        out.emit(
            json!({ "update": true, "version": update::CURRENT, "latest": release.version, "page": release.page }),
            || {
                out.note(&format!(
                    "Riven {} is out (this is {}): {}",
                    release.version,
                    update::CURRENT,
                    release.page
                ));
                if !install.self_updates() {
                    out.note("this copy is managed by a package manager or Nix; update it there");
                }
            },
        );
        return Ok(ExitCode::SUCCESS);
    }
    let bar = out.spinner(format!("Downloading Riven {}", release.version));
    let progress = |done: u64, total: u64| {
        bar.set_message(format!(
            "Downloading Riven {} ({:.1}/{:.1} MB)",
            release.version,
            done as f64 / 1e6,
            total as f64 / 1e6
        ))
    };
    update::install(&release, &install, EDITION, progress).await?;
    bar.finish_and_clear();
    out.emit(
        json!({ "updated": true, "from": update::CURRENT, "version": release.version }),
        || {
            out.success(&format!(
                "Updated Riven {} → {}",
                update::CURRENT,
                release.version
            ))
        },
    );
    Ok(ExitCode::SUCCESS)
}
