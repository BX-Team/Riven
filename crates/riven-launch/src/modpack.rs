use std::path::{Path, PathBuf};

use riven_build::import::{ImportKind, ImportStage};
use riven_format::{GitHubPack, Instance, ModrinthPack};
use riven_sources::github::Release as GitHubRelease;
use riven_sources::{Channel, GitHub, Modrinth, Source as _, Version};
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

/// A pack file Riven turns into a `.riven` archive before installing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Riven,
    Mrpack,
    Packwiz,
}

impl Format {
    fn of_name(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        match () {
            _ if name.ends_with(".riven") => Some(Self::Riven),
            _ if name.ends_with(".mrpack") => Some(Self::Mrpack),
            _ if name.ends_with("pack.toml") => Some(Self::Packwiz),
            _ => None,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Riven => "riven",
            Self::Mrpack => "mrpack",
            Self::Packwiz => "toml",
        }
    }
}

/// What a link points at, read from its shape alone.
#[derive(Debug, PartialEq, Eq)]
enum Shape {
    /// A Riven channel link, `gh:owner/repo` or a local archive: installed as it is.
    Riven(String),
    Modrinth {
        project: String,
        version: Option<String>,
    },
    GitHub {
        repo: String,
        tag: Option<String>,
        asset: Option<String>,
    },
    File {
        url: String,
        format: Format,
    },
}

fn shape(link: &str) -> Shape {
    let link = link.trim();
    let Ok(url) = url::Url::parse(link) else {
        return Shape::Riven(link.to_owned());
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Shape::Riven(link.to_owned());
    }
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_start_matches("www.");
    let parts: Vec<String> = url
        .path_segments()
        .map(|s| {
            s.filter(|p| !p.is_empty())
                .map(|p| {
                    percent_encoding::percent_decode_str(p)
                        .decode_utf8_lossy()
                        .into_owned()
                })
                .collect()
        })
        .unwrap_or_default();
    let part = |i: usize| parts.get(i).map(String::as_str);
    match host {
        "modrinth.com" if matches!(part(0), Some("modpack" | "project")) && part(1).is_some() => {
            Shape::Modrinth {
                project: parts[1].clone(),
                version: (part(2) == Some("version"))
                    .then(|| part(3).map(str::to_owned))
                    .flatten(),
            }
        }
        "github.com" if parts.len() >= 2 => {
            let repo = format!("{}/{}", parts[0], parts[1].trim_end_matches(".git"));
            let (tag, asset) = match (part(2), part(3)) {
                (Some("releases"), Some("tag")) => (part(4).map(str::to_owned), None),
                (Some("releases"), Some("download")) => {
                    (part(4).map(str::to_owned), part(5).map(str::to_owned))
                }
                _ => (None, None),
            };
            Shape::GitHub { repo, tag, asset }
        }
        _ => match parts.last().and_then(|name| Format::of_name(name)) {
            Some(format) => Shape::File {
                url: link.to_owned(),
                format,
            },
            None => Shape::Riven(link.to_owned()),
        },
    }
}

/// A pack a link leads to, ready to preview and install.
#[derive(Debug, Clone, Default)]
pub struct Linked {
    /// A Riven link or a local `.riven` archive.
    pub source: String,
    pub title: Option<String>,
    pub icon_url: Option<String>,
    pub modrinth: Option<ModrinthPack>,
    pub github: Option<GitHubPack>,
}

/// Follows a link to a pack: Riven links stay as they are; Modrinth pages, GitHub releases and
/// `.mrpack`, `.riven` or packwiz `pack.toml` files are fetched and converted first.
pub async fn resolve(link: &str, stage: impl Fn(ImportStage)) -> Result<Linked, LaunchError> {
    match shape(link) {
        Shape::Riven(source) => Ok(Linked {
            source,
            ..Linked::default()
        }),
        Shape::File { url, format } => {
            let stem = url
                .rsplit('/')
                .find(|p| !p.is_empty() && *p != "pack.toml")
                .map(|p| p.trim_end_matches(".mrpack").trim_end_matches(".riven"))
                .unwrap_or("pack");
            Ok(Linked {
                source: archive(&url, format, &file_stem(stem), &stage).await?,
                ..Linked::default()
            })
        }
        Shape::Modrinth { project, version } => {
            let modrinth = Modrinth::new(riven_sources::client());
            let info = modrinth.project(&project).await.map_err(download)?;
            let versions = modrinth
                .project_versions(&info.id)
                .await
                .map_err(download)?;
            let picked = match &version {
                Some(v) => versions.iter().find(|x| x.id == *v || x.number == *v),
                None => versions
                    .iter()
                    .find(|x| x.channel == Channel::Release)
                    .or(versions.first()),
            }
            .ok_or_else(|| LaunchError::Download(format!("{project}: no such version")))?;
            let url = mrpack_url(picked)?;
            let stem = file_stem(&format!("{}-{}", info.id, picked.id));
            Ok(Linked {
                source: archive(&url, Format::Mrpack, &stem, &stage).await?,
                title: Some(info.title),
                icon_url: info.icon_url,
                modrinth: Some(ModrinthPack {
                    project: info.id,
                    version: picked.id.clone(),
                }),
                github: None,
            })
        }
        Shape::GitHub { repo, tag, asset } => {
            let github = GitHub::new(riven_sources::client());
            let found = match &tag {
                Some(tag) => github
                    .release(&repo, tag)
                    .await
                    .map(|r| pick_asset(&r, asset.as_deref()).map(|a| (r, a))),
                None => github
                    .releases(&repo)
                    .await
                    .map(|all| latest_with_pack(&all, true, None)),
            };
            let (release, (_, url, format)) = match found {
                Ok(Some(found)) => found,
                // A repository without pack assets may still be a pack deployed to GitHub Pages.
                Ok(None) | Err(_) if tag.is_none() => {
                    return Ok(Linked {
                        source: format!("gh:{repo}"),
                        ..Linked::default()
                    });
                }
                Ok(None) => {
                    return Err(LaunchError::Download(format!(
                        "{repo}: the release has no .mrpack or .riven"
                    )));
                }
                Err(e) => return Err(download(e)),
            };
            let stem = file_stem(&format!("{}-{}", repo.replace('/', "-"), release.tag));
            Ok(Linked {
                source: archive(&url, format, &stem, &stage).await?,
                title: None,
                icon_url: None,
                modrinth: None,
                github: Some(GitHubPack {
                    repo,
                    tag: release.tag,
                    kind: format.extension().into(),
                }),
            })
        }
    }
}

fn download(e: impl std::fmt::Display) -> LaunchError {
    LaunchError::Download(e.to_string())
}

fn mrpack_url(version: &Version) -> Result<String, LaunchError> {
    version
        .primary_file()
        .filter(|f| f.filename.ends_with(".mrpack"))
        .and_then(|f| f.url.clone())
        .ok_or_else(|| LaunchError::Download(format!("{}: not a modpack", version.number)))
}

/// A file name part made of letters, digits, `-`, `_` and `.` only.
fn file_stem(raw: &str) -> String {
    let stem: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches(['-', '.']);
    if stem.is_empty() {
        "pack".into()
    } else {
        stem.into()
    }
}

/// The asset a release ships its pack as: the named one, else the first `.mrpack`, else `.riven`.
fn pick_asset(release: &GitHubRelease, name: Option<&str>) -> Option<(String, String, Format)> {
    let assets = || release.assets.iter();
    let asset = match name {
        Some(name) => assets().find(|a| a.name == name),
        None => assets()
            .find(|a| Format::of_name(&a.name) == Some(Format::Mrpack))
            .or_else(|| assets().find(|a| Format::of_name(&a.name) == Some(Format::Riven))),
    }?;
    let format = Format::of_name(&asset.name).filter(|f| *f != Format::Packwiz)?;
    Some((release.name.clone(), asset.url.clone(), format))
}

type Picked = (GitHubRelease, (String, String, Format));

/// The newest release carrying a pack (of `format`, when given); pre-releases only when allowed.
fn latest_with_pack(
    releases: &[GitHubRelease],
    allow_pre: bool,
    format: Option<Format>,
) -> Option<Picked> {
    let with_pack = |pre: bool| {
        releases
            .iter()
            .filter(|r| pre || !r.prerelease)
            .find_map(|r| {
                let asset = pick_asset(r, None)?;
                format
                    .is_none_or(|f| f == asset.2)
                    .then(|| (r.clone(), asset))
            })
    };
    with_pack(false).or_else(|| allow_pre.then(|| with_pack(true)).flatten())
}

/// Fetches a pack file into the packs folder as a `.riven` archive named `stem`.
async fn archive(
    source: &str,
    format: Format,
    stem: &str,
    stage: &impl Fn(ImportStage),
) -> Result<String, LaunchError> {
    let out = packs_dir()?.join(format!("{stem}.riven"));
    match format {
        Format::Riven => {
            stage(ImportStage::Reading);
            let response = riven_sources::client()
                .get(source)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(download)?;
            let bytes = response.bytes().await.map_err(download)?;
            let dir = packs_dir()?;
            std::fs::create_dir_all(&dir).map_err(crate::io(&dir))?;
            std::fs::write(&out, &bytes).map_err(crate::io(&out))?;
        }
        Format::Mrpack | Format::Packwiz => {
            let kind = if format == Format::Mrpack {
                ImportKind::Mrpack
            } else {
                ImportKind::Packwiz
            };
            riven_build::ship::archive_import(kind, source, &out, stage)
                .await
                .map_err(download)?;
        }
    }
    Ok(out.display().to_string())
}

/// A newer version of the modpack an instance follows outside Riven channels.
#[derive(Debug, Clone)]
pub struct Newer {
    /// The version number or release name shown to the player.
    pub number: String,
    from: Upstream,
}

#[derive(Debug, Clone)]
enum Upstream {
    Modrinth(Box<Version>),
    GitHub {
        pack: GitHubPack,
        url: String,
        format: Format,
    },
}

/// Whether the instance's pack comes from Modrinth or a GitHub release, which [`newer`] checks.
pub fn follows(instance: &Instance) -> bool {
    instance.modrinth.is_some() || instance.github.is_some()
}

/// The newest version of the instance's modpack, at least as stable as the installed one, when it is newer.
pub async fn newer(instance: &Instance) -> Result<Option<Newer>, LaunchError> {
    if let Some(pack) = &instance.modrinth {
        return newer_on_modrinth(pack).await;
    }
    let Some(pack) = &instance.github else {
        return Ok(None);
    };
    let releases = GitHub::new(riven_sources::client())
        .releases(&pack.repo)
        .await
        .map_err(download)?;
    let installed = releases.iter().find(|r| r.tag == pack.tag);
    let format = match pack.kind.as_str() {
        "riven" => Format::Riven,
        _ => Format::Mrpack,
    };
    let allow_pre = installed.is_none_or(|r| r.prerelease);
    let Some((latest, (name, url, format))) = latest_with_pack(&releases, allow_pre, Some(format))
    else {
        return Ok(None);
    };
    let newer = latest.tag != pack.tag
        && installed.is_none_or(|i| {
            crate::parse_rfc3339(&latest.published) > crate::parse_rfc3339(&i.published)
        });
    Ok(newer.then(|| Newer {
        number: name,
        from: Upstream::GitHub {
            pack: GitHubPack {
                repo: pack.repo.clone(),
                tag: latest.tag.clone(),
                kind: pack.kind.clone(),
            },
            url,
            format,
        },
    }))
}

async fn newer_on_modrinth(pack: &ModrinthPack) -> Result<Option<Newer>, LaunchError> {
    let versions = Modrinth::new(riven_sources::client())
        .project_versions(&pack.project)
        .await
        .map_err(download)?;
    let installed = versions.iter().find(|v| v.id == pack.version);
    let stable = installed.map_or(Channel::Release, |v| v.channel);
    let Some(latest) = versions.iter().find(|v| v.channel <= stable) else {
        return Ok(None);
    };
    let newer = latest.id != pack.version
        && installed.is_none_or(|i| {
            crate::parse_rfc3339(&latest.published) > crate::parse_rfc3339(&i.published)
        });
    Ok(newer.then(|| Newer {
        number: latest.number.clone(),
        from: Upstream::Modrinth(Box::new(latest.clone())),
    }))
}

/// Converts the newer version and installs it over the instance's pack.
pub async fn update(
    store: &Instances,
    id: &str,
    newer: Newer,
    report: impl Fn(Progress) + Send + Sync + 'static,
) -> Result<(), LaunchError> {
    let instance = store.load(id)?;
    report(Progress::Stage(Stage::Convert));
    let archive = match &newer.from {
        Upstream::Modrinth(version) => {
            let project = instance
                .modrinth
                .as_ref()
                .map(|m| m.project.clone())
                .ok_or_else(|| LaunchError::NoInstance(id.to_owned()))?;
            let stem = file_stem(&format!("{project}-{}", version.id));
            archive(&mrpack_url(version)?, Format::Mrpack, &stem, &|_| {}).await?
        }
        Upstream::GitHub { pack, url, format } => {
            let stem = file_stem(&format!("{}-{}", pack.repo.replace('/', "-"), pack.tag));
            archive(url, *format, &stem, &|_| {}).await?
        }
    };
    let archive = PathBuf::from(archive);
    let old = install::load_state(&store.game_dir(id))?.map(|s| PathBuf::from(s.source));
    let request = Request {
        source: Some(archive.display().to_string()),
        replaces: true,
        ..Request::default()
    };
    game::install_pack(store, id, request, report).await?;
    let mut instance = store.load(id)?;
    match newer.from {
        Upstream::Modrinth(version) => {
            if let Some(m) = &mut instance.modrinth {
                m.version = version.id;
            }
        }
        Upstream::GitHub { pack, .. } => instance.github = Some(pack),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn github(repo: &str, tag: Option<&str>, asset: Option<&str>) -> Shape {
        Shape::GitHub {
            repo: repo.into(),
            tag: tag.map(Into::into),
            asset: asset.map(Into::into),
        }
    }

    #[test]
    fn links_are_told_apart_by_shape() {
        assert_eq!(
            shape("https://modrinth.com/modpack/fabulously-optimized"),
            Shape::Modrinth {
                project: "fabulously-optimized".into(),
                version: None
            }
        );
        assert_eq!(
            shape("https://modrinth.com/modpack/fo/version/6.4.0"),
            Shape::Modrinth {
                project: "fo".into(),
                version: Some("6.4.0".into())
            }
        );
        assert_eq!(
            shape("https://github.com/BX-Team/VideCraft"),
            github("BX-Team/VideCraft", None, None)
        );
        assert_eq!(
            shape("https://github.com/o/r.git"),
            github("o/r", None, None)
        );
        assert_eq!(
            shape("https://github.com/o/r/releases/tag/v1.2"),
            github("o/r", Some("v1.2"), None)
        );
        assert_eq!(
            shape("https://github.com/o/r/releases/download/v1.2/My%20Pack.mrpack"),
            github("o/r", Some("v1.2"), Some("My Pack.mrpack"))
        );
        assert_eq!(
            shape("https://example.com/packs/pack.toml"),
            Shape::File {
                url: "https://example.com/packs/pack.toml".into(),
                format: Format::Packwiz
            }
        );
        assert!(matches!(
            shape("https://cdn.example.com/a/b.mrpack?x=1"),
            Shape::File {
                format: Format::Mrpack,
                ..
            }
        ));
        assert_eq!(
            shape("https://bx-team.github.io/pack/channels/stable.json"),
            Shape::Riven("https://bx-team.github.io/pack/channels/stable.json".into())
        );
        assert_eq!(shape(" gh:o/r "), Shape::Riven("gh:o/r".into()));
        assert_eq!(
            shape("/home/me/pack.riven"),
            Shape::Riven("/home/me/pack.riven".into())
        );
    }

    #[test]
    fn stems_are_safe_file_names() {
        assert_eq!(file_stem("BX-Team/pack v1.0"), "BX-Team-pack-v1.0");
        assert_eq!(file_stem("../.."), "pack");
    }
}
