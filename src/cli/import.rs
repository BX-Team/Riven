use std::process::ExitCode;

use anyhow::Context;
use riven_build::import::{ImportKind, ImportStage, Imported, import_pack};
use riven_format::SourceKind;
use serde_json::json;

use super::ImportFormat;
use super::output::Output;

pub async fn import(out: &Output, format: ImportFormat, source: &str) -> anyhow::Result<ExitCode> {
    let dir = std::env::current_dir().context("cannot read the current directory")?;
    let kind = match format {
        ImportFormat::Mrpack => ImportKind::Mrpack,
        ImportFormat::Packwiz => ImportKind::Packwiz,
    };
    let spinner = out.spinner(format!("Reading {source}"));
    let imported = import_pack(&dir, kind, source, |stage| {
        spinner.set_message(match stage {
            ImportStage::Reading => format!("Reading {source}"),
            ImportStage::Identifying { files } => format!("Identifying {files} files"),
            ImportStage::Downloading => "Downloading files that are not on Modrinth".into(),
            ImportStage::Writing => "Writing the project".into(),
        })
    })
    .await;
    spinner.finish_and_clear();
    let Imported {
        workspace,
        overrides,
        unmatched,
    } = imported?;
    let project = workspace.project;

    let count = |kind: SourceKind| {
        project
            .content
            .iter()
            .filter(|e| e.source.kind() == kind)
            .count()
    };
    let ids_of = |kind: SourceKind| -> Vec<&str> {
        project
            .content
            .iter()
            .filter(|e| e.source.kind() == kind)
            .map(|e| e.id.as_str())
            .collect()
    };
    out.emit(
        json!({
            "project": project,
            "overrides": overrides,
            "url": ids_of(SourceKind::Url),
            "local": ids_of(SourceKind::Local),
            "unmatched": unmatched,
        }),
        || {
            if !unmatched.is_empty() {
                out.warn(&format!(
                    "{} files were not identified and were skipped:",
                    unmatched.len()
                ));
                for path in &unmatched {
                    eprintln!("  {path}");
                }
            }
            for id in ids_of(SourceKind::Url) {
                out.note(&format!("`{id}` is not on Modrinth; kept as a URL source"));
            }
            for id in ids_of(SourceKind::Local) {
                out.note(&format!("`{id}` is not on Modrinth; copied into local/"));
            }
            out.success(&format!(
                "Imported `{}` {}: {} entries ({} Modrinth), {} override files",
                project.name,
                project.version,
                project.content.len(),
                count(SourceKind::Modrinth),
                overrides
            ));
        },
    );
    Ok(ExitCode::SUCCESS)
}
