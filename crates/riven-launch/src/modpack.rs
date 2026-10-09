use std::path::{Path, PathBuf};

use riven_format::ModrinthPack;
use riven_sources::{Channel, Modrinth, Version};
use riven_sync::install;
use riven_sync::update::Request;

use crate::LaunchError;
use crate::game::{self, Progress, Stage};
use crate::instances::Instances;

/// Where converted `.mrpack` archives live.
pub fn packs_dir() -> Result<PathBuf, LaunchError> {
    riven_sync::data_dir()
        .map(|d| d.join("packs"))
        .ok_or(LaunchError::NoDir("data"))
}

/// The newest version of the modpack, at least as stable as the installed one, when it is newer.
pub async fn newer(pack: &ModrinthPack) -> Result<Option<Version>, LaunchError> {
    let versions = Modrinth::new(riven_sources::client())
        .project_versions(&pack.project)
        .await
        .map_err(|e| LaunchError::Download(e.to_string()))?;
    let installed = versions.iter().find(|v| v.id == pack.version);
    let stable = installed.map_or(Channel::Release, |v| v.channel);
    let Some(latest) = versions.iter().find(|v| v.channel <= stable) else {
        return Ok(None);
    };
    let newer = latest.id != pack.version
        && installed.is_none_or(|i| {
            crate::parse_rfc3339(&latest.published) > crate::parse_rfc3339(&i.published)
        });
    Ok(newer.then(|| latest.clone()))
}

/// Converts `version`'s `.mrpack` and installs it over the instance's pack.
pub async fn update(
    store: &Instances,
    id: &str,
    version: Version,
    report: impl Fn(Progress) + Send + Sync + 'static,
) -> Result<(), LaunchError> {
    let mut instance = store.load(id)?;
    let project = instance
        .modrinth
        .as_ref()
        .map(|m| m.project.clone())
        .ok_or_else(|| LaunchError::NoInstance(id.to_owned()))?;
    let url = version
        .primary_file()
        .and_then(|f| f.url.clone())
        .ok_or_else(|| LaunchError::Download(version.number.clone()))?;
    report(Progress::Stage(Stage::Pack));
    let archive = packs_dir()?.join(format!("{project}-{}.riven", version.id));
    riven_build::ship::archive_mrpack(&url, &archive, |_| {})
        .await
        .map_err(|e| LaunchError::Download(e.to_string()))?;
    let old = install::load_state(&store.game_dir(id))?.map(|s| PathBuf::from(s.source));
    let request = Request {
        source: Some(archive.display().to_string()),
        replaces: true,
        ..Request::default()
    };
    game::install_pack(store, id, request, report).await?;
    instance = store.load(id)?;
    instance.modrinth = Some(ModrinthPack {
        project,
        version: version.id,
    });
    store.save(id, &instance)?;
    if let Some(old) = old.filter(|o| *o != archive) {
        forget_archive(store, &old);
    }
    Ok(())
}

/// Deletes a converted archive no instance installs from anymore.
fn forget_archive(store: &Instances, archive: &Path) {
    let Ok(dir) = packs_dir() else {
        return;
    };
    if archive.parent() != Some(dir.as_path()) {
        return;
    }
    let used = store.list().unwrap_or_default().iter().any(|(id, _)| {
        install::load_state(&store.game_dir(id))
            .ok()
            .flatten()
            .is_some_and(|s| riven_sync::remote::same_pack(&s.source, &archive.to_string_lossy()))
    });
    if !used && let Err(e) = std::fs::remove_file(archive) {
        tracing::debug!("{}: {e}", archive.display());
    }
}
