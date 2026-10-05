use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{StreamExt as _, stream};
use gpui_kit::SharedString;
use riven_launch::mods::{ContentFile, Details};
use riven_sources::{Cache, Known, Modrinth};

const PARALLEL_ICONS: usize = 16;

/// One file of an instance's `mods/`, with what reading it and Modrinth told about it.
#[derive(Clone)]
pub struct ModRow {
    pub file: ContentFile,
    pub details: Option<Details>,
    pub title: SharedString,
    pub version: SharedString,
    pub icon: Option<PathBuf>,
}

impl ModRow {
    fn new(file: ContentFile) -> Self {
        let stem = file
            .name
            .trim_end_matches(".jar")
            .trim_end_matches(".zip")
            .to_owned();
        Self {
            file,
            details: None,
            title: stem.into(),
            version: SharedString::default(),
            icon: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Modified,
}

/// The rows of the mods tab with the current filter and sort applied.
#[derive(Default)]
pub struct ModsTable {
    rows: Vec<ModRow>,
    shown: Vec<usize>,
    filter: String,
    sort: Option<(SortBy, bool)>,
}

impl ModsTable {
    pub fn rows(&self) -> &[ModRow] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn get(&self, ix: usize) -> Option<&ModRow> {
        self.shown.get(ix).map(|&i| &self.rows[i])
    }

    pub fn sort(&self) -> Option<(SortBy, bool)> {
        self.sort
    }

    pub fn set_rows(&mut self, mut rows: Vec<ModRow>) {
        rows.sort_by_cached_key(|r| r.title.to_lowercase());
        self.rows = rows;
        self.apply();
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_lowercase();
        self.apply();
    }

    /// Sorts by `by`; asking for the same column again flips the direction.
    pub fn toggle_sort(&mut self, by: SortBy) {
        self.sort = match self.sort {
            Some((current, desc)) if current == by => Some((by, !desc)),
            _ => Some((by, by == SortBy::Modified)),
        };
        self.apply();
    }

    fn apply(&mut self) {
        let needle = &self.filter;
        let rows = &self.rows;
        self.shown = (0..rows.len())
            .filter(|&i| {
                needle.is_empty()
                    || rows[i].title.to_lowercase().contains(needle)
                    || rows[i].file.name.to_lowercase().contains(needle)
            })
            .collect();
        if let Some((by, desc)) = self.sort {
            match by {
                SortBy::Name => self.shown.sort_by_key(|&i| rows[i].title.to_lowercase()),
                SortBy::Modified => self.shown.sort_by_key(|&i| rows[i].file.modified),
            }
            if desc {
                self.shown.reverse();
            }
        }
    }
}

/// Lists `mods/`, named after the files until their metadata is read.
pub fn scan_rows(game_dir: &Path) -> Vec<ModRow> {
    riven_launch::mods::scan(game_dir, "mods")
        .unwrap_or_else(|e| {
            tracing::warn!("{e}");
            vec![]
        })
        .into_iter()
        .map(ModRow::new)
        .collect()
}

/// Hashes the files and reads each jar's own name and version; unchanged files come from a cache.
pub fn describe(game_dir: &Path, mut rows: Vec<ModRow>) -> Vec<ModRow> {
    let files: Vec<ContentFile> = rows.iter().map(|r| r.file.clone()).collect();
    let found = riven_launch::mods::details_cached(game_dir, &files);
    for (row, details) in rows.iter_mut().zip(found) {
        let Some(details) = details else {
            continue;
        };
        if let Some(title) = &details.title {
            row.title = title.clone().into();
        }
        row.version = details.version.clone().unwrap_or_default().into();
        row.details = Some(details);
    }
    rows
}

/// What Modrinth knows about a file: the project's title and its cached icon.
pub struct Identified {
    pub title: String,
    pub icon: Option<PathBuf>,
}

fn cache_dir() -> Option<PathBuf> {
    riven_sync::data_dir().map(|d| d.join("cache"))
}

/// Looks the files up on Modrinth by hash and downloads the icons it names.
pub async fn identify(hashes: Vec<(String, String)>) -> HashMap<String, Identified> {
    let Some(cache) = cache_dir() else {
        return HashMap::new();
    };
    let modrinth = Modrinth::new(riven_sources::client()).with_cache(Cache::new(
        cache.join("api"),
        Duration::from_secs(24 * 3600),
    ));
    let known: Vec<Known> = hashes
        .iter()
        .map(|(sha512, sha1)| Known {
            sha512: Some(sha512.clone()),
            sha1: Some(sha1.clone()),
        })
        .collect();
    let matched = match riven_sources::identify(&modrinth, &known).await {
        Ok(matched) => matched,
        Err(e) => {
            tracing::warn!("cannot look mods up on Modrinth: {e}");
            return HashMap::new();
        }
    };
    let icons = cache.join("icons");
    stream::iter(hashes.into_iter().zip(matched))
        .filter_map(|((sha512, _), m)| async move { Some((sha512, m?)) })
        .map(|(sha512, m)| {
            let icons = icons.clone();
            async move {
                let icon = match &m.info.icon_url {
                    Some(url) => fetch_icon(&icons, &m.info.id, url).await,
                    None => None,
                };
                let title = m.info.title.clone();
                (sha512, Identified { title, icon })
            }
        })
        .buffer_unordered(PARALLEL_ICONS)
        .collect()
        .await
}

/// Side of the icons drawn in lists; full-size and animated originals cost every frame.
const ICON_PX: u32 = 64;

/// A small still copy of an icon: the first frame, scaled down, as PNG.
fn thumbnail(bytes: &[u8], out: &Path) -> Option<()> {
    let image = image::load_from_memory(bytes).ok()?;
    let small = image.thumbnail(ICON_PX, ICON_PX);
    small.save_with_format(out, image::ImageFormat::Png).ok()
}

/// The icon of a Modrinth project as a cached thumbnail, downloaded once.
pub async fn fetch_icon(dir: &Path, project: &str, url: &str) -> Option<PathBuf> {
    let thumb = dir.join(format!("{project}@{ICON_PX}.png"));
    if thumb.is_file() {
        return Some(thumb);
    }
    let response = riven_sources::client()
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .ok()?;
    let bytes = response.bytes().await.ok()?;
    std::fs::create_dir_all(dir).ok()?;
    let out = thumb.clone();
    tokio::task::spawn_blocking(move || thumbnail(&bytes, &out))
        .await
        .ok()??;
    Some(thumb)
}
