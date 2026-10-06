use std::collections::HashSet;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, anyhow, bail};
use futures_util::{StreamExt, stream};
use riven_build::import::{PackFile, Resolution, build_project, extract};
use riven_build::packwiz::Packwiz;
use riven_format::{PackPath, SourceKind};
use riven_resolve::Downloader;
use riven_sources::{Cache, Known, Modrinth};
use serde_json::json;

use super::ImportFormat;
use super::output::Output;
use super::project::{PROJECT_FILE, StoreJars, ensure_gitignore, game_meta, write_atomic};

const PARALLEL_DOWNLOADS: usize = 8;

async fn read_source(source: &str) -> anyhow::Result<Vec<u8>> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let response = riven_sources::client()
            .get(source)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .with_context(|| format!("cannot download {source}"))?;
        Ok(response.bytes().await?.to_vec())
    } else {
        std::fs::read(source).with_context(|| format!("cannot read {source}"))
    }
}

/// Reads a packwiz pack from a directory, a `pack.toml` path, or a `pack.toml` URL.
async fn read_packwiz(source: &str) -> anyhow::Result<Packwiz> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let mut base = url::Url::parse(source).with_context(|| format!("bad URL {source}"))?;
        if !base.path().ends_with(".toml") && !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let http = riven_sources::client();
        let pack = Packwiz::read(|path| {
            let url = base.join(&path);
            let http = http.clone();
            async move {
                let url = url.map_err(|e| e.to_string())?;
                let response = http
                    .get(url)
                    .send()
                    .await
                    .and_then(reqwest::Response::error_for_status)
                    .map_err(|e| e.to_string())?;
                let bytes = response.bytes().await.map_err(|e| e.to_string())?;
                Ok(bytes.to_vec())
            }
        })
        .await?;
        return Ok(pack);
    }
    let path = Path::new(source);
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(Path::new("."))
    };
    let pack = Packwiz::read(|file| {
        let path = dir.join(file);
        async move { std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display())) }
    })
    .await?;
    Ok(pack)
}

/// Downloads files kept as URL sources whose sha512 or size the pack does not record.
async fn complete_urls(
    jars: &StoreJars,
    files: &mut [PackFile],
    resolved: &[Resolution],
) -> anyhow::Result<()> {
    let wanted: Vec<(usize, String)> = resolved
        .iter()
        .enumerate()
        .filter_map(|(i, r)| match r {
            Resolution::Url(url) if files[i].known.sha512.is_none() || files[i].size == 0 => {
                Some((i, url.clone()))
            }
            _ => None,
        })
        .collect();
    let fetched: Vec<_> = stream::iter(wanted)
        .map(|(i, url)| async move { (i, url.clone(), jars.download(&url).await) })
        .buffer_unordered(PARALLEL_DOWNLOADS)
        .collect()
        .await;
    for (i, url, result) in fetched {
        let file = &mut files[i];
        let got = result.map_err(|e| anyhow!("cannot download {url}: {e}"))?;
        if let Some(sha1) = &file.known.sha1
            && got.hashes.sha1.as_deref() != Some(sha1.as_str())
        {
            bail!(
                "{url} does not match the sha1 the pack lists for {}",
                file.path
            );
        }
        file.known.sha512 = got.hashes.sha512;
        file.known.sha1 = got.hashes.sha1;
        file.size = got.size;
    }
    Ok(())
}

/// A `local/` path for an unidentified embedded file, unique among those taken.
fn local_copy(file: &PackFile, taken: &mut HashSet<String>) -> anyhow::Result<PackPath> {
    let name = file.path.file_name().to_owned();
    let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
    let candidate = (1..)
        .map(|n| match n {
            1 => name.clone(),
            n => format!("{stem}-{n}.{ext}"),
        })
        .find(|c| !taken.contains(c))
        .expect("some suffix is free");
    taken.insert(candidate.clone());
    Ok(PackPath::new(format!("local/{candidate}"))?)
}

pub async fn import(out: &Output, format: ImportFormat, source: &str) -> anyhow::Result<ExitCode> {
    let dir = std::env::current_dir().context("cannot read the current directory")?;
    if dir.join(PROJECT_FILE).exists() {
        bail!("{PROJECT_FILE} already exists here");
    }

    let spinner = out.spinner(format!("Reading {source}"));
    let (archive, info, overrides, mut files, preserve) = match format {
        ImportFormat::Mrpack => {
            let bytes = read_source(source).await?;
            let pack = riven_build::mrpack::Mrpack::read(&bytes)?;
            (
                Some(bytes),
                pack.info()?,
                pack.overrides,
                pack.files,
                vec![],
            )
        }
        ImportFormat::Packwiz => {
            let pack = read_packwiz(source).await?;
            (None, pack.info, pack.overrides, pack.files, pack.preserve)
        }
    };

    let cache = Cache::new(
        dir.join(".riven").join("cache"),
        std::time::Duration::from_secs(300),
    );
    let modrinth = Modrinth::new(riven_sources::client()).with_cache(cache.clone());
    spinner.set_message(format!("Identifying {} files", files.len()));
    let known: Vec<Known> = files.iter().map(|f| f.known.clone()).collect();
    let matched = riven_sources::identify(&modrinth, &known).await?;
    let java = game_meta(Some(cache)).java_major(&info.minecraft).await?;
    spinner.finish_and_clear();

    let mut unmatched = Vec::new();
    let mut taken = HashSet::new();
    let mut copies = Vec::new();
    let mut resolved = Vec::new();
    for (file, found) in files.iter().zip(matched) {
        resolved.push(match found {
            Some(m) => Resolution::Platform(Box::new(m)),
            None if file.archive_name.is_some() => {
                let local = local_copy(file, &mut taken)?;
                copies.push((file.archive_name.clone().unwrap_or_default(), local.clone()));
                Resolution::Local(local)
            }
            None if file.archive_name.is_none() && !file.downloads.is_empty() => {
                Resolution::Url(file.downloads[0].clone())
            }
            None => {
                unmatched.push(file.path.to_string());
                Resolution::Skipped
            }
        });
    }
    if resolved.iter().any(|r| matches!(r, Resolution::Url(_))) {
        let spinner = out.spinner("Downloading files that are not on Modrinth");
        complete_urls(&StoreJars::new(&dir)?, &mut files, &resolved).await?;
        spinner.finish_and_clear();
    }
    let mut project = build_project(&info, java, &files, &resolved)?;
    project.files.preserve = preserve;

    for file in &overrides {
        let target = dir
            .join("overrides")
            .join(file.scope.dir())
            .join(file.path.as_str());
        if target.exists() {
            bail!("{} already exists", target.display());
        }
        let parent = target.parent().expect("override path has a parent");
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        std::fs::write(&target, &file.bytes)
            .with_context(|| format!("cannot write {}", target.display()))?;
    }
    for (archive_name, local) in &copies {
        let target = dir.join(local.as_str());
        std::fs::create_dir_all(target.parent().expect("local path has a parent"))?;
        let archive = archive.as_deref().context("only archives embed files")?;
        std::fs::write(&target, extract(archive, archive_name)?)
            .with_context(|| format!("cannot write {}", target.display()))?;
    }
    write_atomic(&dir.join(PROJECT_FILE), &riven_format::to_string(&project))?;
    ensure_gitignore(&dir)?;

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
            "overrides": overrides.len(),
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
                overrides.len()
            ));
        },
    );
    Ok(ExitCode::SUCCESS)
}
