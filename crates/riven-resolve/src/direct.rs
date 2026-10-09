use std::future::Future;
use std::io::Cursor;

use riven_format::{
    Entry, EntryFile, Hashes, Kind, LoaderKind, PackPath, Project, Reason, Side,
    Source as EntrySource, UpdatePolicy,
};
use riven_sources::github::{Asset, Release};
use riven_sources::{GitHub, Matched, loader_name};

use crate::check::fits_game;
use crate::meta::JarMeta;
use crate::plan::{ResolveError, kind_dir, pack_env};

/// Release jars downloaded at most while looking for one that fits the pack's game.
const MAX_CANDIDATES: usize = 12;
/// Asset name pieces that mark a build nobody installs.
const SKIPPED_ASSETS: &[&str] = &["sources", "javadoc", "dev", "api", "slim"];

/// A downloaded file with the hashes computed while storing it.
#[derive(Debug, Clone)]
pub struct Downloaded {
    pub bytes: Vec<u8>,
    pub hashes: Hashes,
    pub size: u64,
}

/// Downloads files whose hashes the pack does not know yet.
pub trait Downloader: Sync {
    fn download(&self, url: &str) -> impl Future<Output = Result<Downloaded, String>> + Send;
}

/// The content kind of a file, from its extension and, for zips, its layout.
pub fn detect_kind(filename: &str, bytes: &[u8]) -> Option<Kind> {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".jar") {
        return Some(Kind::Mod);
    }
    if !lower.ends_with(".zip") {
        return None;
    }
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let names: Vec<_> = zip.file_names().collect::<Result<_, _>>().ok()?;
    let has_dir = |dir: &str| names.iter().any(|n| n.starts_with(dir));
    if has_dir("shaders/") {
        Some(Kind::ShaderPack)
    } else if names.iter().any(|n| n == "pack.mcmeta") {
        if has_dir("data/") && !has_dir("assets/") {
            Some(Kind::DataPack)
        } else {
            Some(Kind::ResourcePack)
        }
    } else {
        None
    }
}

fn slug(raw: &str) -> String {
    let lowered: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = lowered
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() { "file".into() } else { slug }
}

/// An entry for a file from GitHub, a URL or the pack repository; name and side come from the jar.
pub fn direct_entry(
    pack: &Project,
    source: EntrySource,
    filename: &str,
    file: &Downloaded,
    url: Option<String>,
) -> Result<Entry, ResolveError> {
    let kind = detect_kind(filename, &file.bytes)
        .ok_or_else(|| ResolveError::UnknownFileType(filename.to_owned()))?;
    let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
    let meta = match kind {
        Kind::Mod => JarMeta::read(&file.bytes).ok(),
        _ => None,
    };
    let top = meta.as_ref().and_then(|meta| {
        meta.mods
            .iter()
            .find(|m| m.platform.runs_on(pack.loader.kind))
            .or(meta.mods.first())
    });
    let path = PackPath::new(format!("{}/{filename}", kind_dir(kind))).map_err(|source| {
        ResolveError::BadFileName {
            name: filename.to_owned(),
            source,
        }
    })?;
    Ok(Entry {
        id: slug(top.map_or(stem, |m| m.id.as_str())),
        kind,
        name: top
            .and_then(|m| m.name.clone())
            .unwrap_or_else(|| stem.to_owned()),
        source,
        file: EntryFile {
            path,
            size: file.size,
            hashes: file.hashes.clone(),
            url,
        },
        side: top.and_then(|m| m.side).unwrap_or(Side::Both),
        group: None,
        update: UpdatePolicy::Follow,
        reason: Reason::Explicit,
        requires: Vec::new(),
    })
}

/// Matches `name` against a pattern where `*` stands for any run of characters.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return pattern == name;
    };
    if name.len() < first.len() + last.len() || !name.starts_with(first) || !name.ends_with(last) {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// A pattern that also matches the same asset of later releases: the version from the tag becomes `*`.
pub fn asset_pattern(asset: &str, tag: &str, minecraft: &str) -> String {
    let version = tag.trim_start_matches(['v', 'V']);
    if !version.is_empty() && version != minecraft && asset.contains(version) {
        return asset.replacen(version, "*", 1);
    }
    let mut pattern = asset.to_owned();
    for piece in tag.split(['-', '+', '_']) {
        let piece = piece
            .trim_start_matches(['v', 'V'])
            .trim_start_matches("mc");
        let versionish = piece.starts_with(|c: char| c.is_ascii_digit());
        if versionish && !minecraft.starts_with(piece) && pattern.contains(piece) {
            pattern = pattern.replacen(piece, "*", 1);
        }
    }
    pattern
}

fn plausible_asset(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(stem) = lower
        .strip_suffix(".jar")
        .or_else(|| lower.strip_suffix(".zip"))
    else {
        return false;
    };
    !stem
        .split(['-', '_', '.', '+'])
        .any(|piece| SKIPPED_ASSETS.contains(&piece))
}

fn names_loader(name: &str, loader: LoaderKind) -> bool {
    let lower = name.to_ascii_lowercase();
    match loader {
        LoaderKind::Forge => lower.contains("forge") && !lower.contains("neoforge"),
        other => lower.contains(loader_name(other)),
    }
}

/// The asset of `release` to install: the one matching `pattern`, else the only plausible one.
fn pick_asset<'r>(
    repo: &str,
    release: &'r Release,
    pattern: Option<&str>,
    pack: &Project,
) -> Result<Option<&'r Asset>, ResolveError> {
    let mut candidates: Vec<&Asset> = release
        .assets
        .iter()
        .filter(|a| match pattern {
            Some(pattern) => glob_match(pattern, &a.name),
            None => plausible_asset(&a.name),
        })
        .collect();
    let narrow = |candidates: &mut Vec<&Asset>, keep: &dyn Fn(&str) -> bool| {
        if candidates.len() > 1 && candidates.iter().any(|a| keep(&a.name)) {
            candidates.retain(|a| keep(&a.name));
        }
    };
    narrow(&mut candidates, &plausible_asset);
    narrow(&mut candidates, &|name| {
        names_loader(name, pack.loader.kind)
    });
    narrow(&mut candidates, &|name| name.contains(&pack.minecraft));
    match candidates.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        many => Err(ResolveError::AmbiguousAsset {
            repo: repo.to_owned(),
            tag: release.tag.clone(),
            assets: many.iter().map(|a| a.name.clone()).collect(),
        }),
    }
}

struct Pick {
    tag: String,
    asset: String,
    url: String,
    pattern: String,
    file: Downloaded,
}

/// The newest of `releases` whose asset matches and runs on the pack's game.
async fn pick_release<D: Downloader>(
    downloader: &D,
    pack: &Project,
    repo: &str,
    releases: &[Release],
    pattern: Option<&str>,
) -> Result<Option<Pick>, ResolveError> {
    let env = pack_env(pack);
    let mut tried = 0;
    for release in releases {
        let Some(asset) = pick_asset(repo, release, pattern, pack)? else {
            continue;
        };
        if tried == MAX_CANDIDATES {
            break;
        }
        tried += 1;
        let file =
            downloader
                .download(&asset.url)
                .await
                .map_err(|message| ResolveError::Download {
                    url: asset.url.clone(),
                    message,
                })?;
        // Releases for several Minecraft versions often share one asset name; the jar decides.
        let fits = !asset.name.to_ascii_lowercase().ends_with(".jar")
            || JarMeta::read(&file.bytes).map_or(true, |meta| fits_game(&env, &meta));
        if fits {
            return Ok(Some(Pick {
                tag: release.tag.clone(),
                asset: asset.name.clone(),
                url: asset.url.clone(),
                pattern: pattern
                    .map(str::to_owned)
                    .unwrap_or_else(|| asset_pattern(&asset.name, &release.tag, &pack.minecraft)),
                file,
            }));
        }
    }
    Ok(None)
}

fn no_release(pack: &Project, repo: &str) -> ResolveError {
    ResolveError::NoRelease {
        repo: repo.to_owned(),
        minecraft: pack.minecraft.clone(),
        loader: loader_name(pack.loader.kind).into(),
    }
}

/// An entry for `repo` (`owner/name`): `tag` or the newest release fitting the pack.
pub async fn github_entry<D: Downloader>(
    github: &GitHub,
    downloader: &D,
    pack: &Project,
    repo: &str,
    tag: Option<&str>,
    pattern: Option<&str>,
) -> Result<Entry, ResolveError> {
    let releases = match tag {
        Some(tag) => vec![github.release(repo, tag).await?],
        None => github.releases(repo).await?,
    };
    let pick = pick_release(downloader, pack, repo, &releases, pattern)
        .await?
        .ok_or_else(|| no_release(pack, repo))?;
    let source = EntrySource::GitHub {
        repo: repo.to_owned(),
        tag: pick.tag,
        asset: pick.pattern,
    };
    direct_entry(pack, source, &pick.asset, &pick.file, Some(pick.url))
}

/// What `entry` becomes after an update: the newest fitting release published after its own.
pub async fn github_update<D: Downloader>(
    github: &GitHub,
    downloader: &D,
    pack: &Project,
    entry: &Entry,
) -> Result<Option<Entry>, ResolveError> {
    let EntrySource::GitHub { repo, tag, asset } = &entry.source else {
        return Ok(None);
    };
    let since = match github.release(repo, tag).await {
        Ok(current) => current.published,
        Err(riven_sources::Error::NotFound(_)) => String::new(),
        Err(e) => return Err(e.into()),
    };
    let newer: Vec<Release> = github
        .releases(repo)
        .await?
        .into_iter()
        .filter(|r| r.published > since && r.tag != *tag)
        .collect();
    let Some(pick) = pick_release(downloader, pack, repo, &newer, Some(asset)).await? else {
        return Ok(None);
    };
    let source = EntrySource::GitHub {
        repo: repo.clone(),
        tag: pick.tag,
        asset: asset.clone(),
    };
    let new = direct_entry(pack, source, &pick.asset, &pick.file, Some(pick.url))?;
    Ok(Some(carry_over(entry, new)))
}

/// `entry` recorded as `matched` instead: the same file, now followed on Modrinth.
pub fn rehome(entry: &Entry, matched: &Matched) -> Entry {
    let platform = matched.version.files.iter().find(|f| {
        (f.hashes.sha512.is_some() && f.hashes.sha512 == entry.file.hashes.sha512)
            || (f.hashes.sha1.is_some() && f.hashes.sha1 == entry.file.hashes.sha1)
    });
    let theirs = platform.map(|f| f.hashes.clone()).unwrap_or_default();
    let ours = &entry.file.hashes;
    let was_url = matches!(entry.source, EntrySource::Url { .. });
    Entry {
        name: matched.info.title.clone(),
        source: matched.source.clone(),
        file: EntryFile {
            path: entry.file.path.clone(),
            size: entry.file.size,
            hashes: Hashes {
                sha512: ours.sha512.clone().or(theirs.sha512),
                sha1: ours.sha1.clone().or(theirs.sha1),
            },
            url: platform.and_then(|f| f.url.clone()),
        },
        // URL entries are pinned only because nothing could update them.
        update: if was_url {
            UpdatePolicy::Follow
        } else {
            entry.update
        },
        ..entry.clone()
    }
}

/// `new` with the author's choices from `old`: id, side, group, update policy and reason.
pub fn carry_over(old: &Entry, new: Entry) -> Entry {
    Entry {
        id: old.id.clone(),
        side: old.side,
        group: old.group.clone(),
        update: old.update,
        reason: old.reason,
        ..new
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::tests::{jar, neoforge_jar};

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.into(),
            size: 1,
            url: format!("https://github.test/{name}"),
        }
    }

    fn release(tag: &str, assets: &[&str]) -> Release {
        Release {
            tag: tag.into(),
            name: tag.into(),
            prerelease: false,
            published: String::new(),
            assets: assets.iter().map(|a| asset(a)).collect(),
        }
    }

    fn pack() -> Project {
        riven_format::from_str(
            r#"{"format":1,"name":"T","id":"t","version":"1.0.0","minecraft":"1.21.1",
                "loader":{"type":"neoforge","version":"21.1.200"},"java":{"major":21}}"#,
        )
        .unwrap()
    }

    #[test]
    fn asset_patterns_replace_the_mod_version_but_keep_minecraft() {
        assert_eq!(
            asset_pattern("modmenu-22.0.0-alpha.1.jar", "v22.0.0-alpha.1", "1.21.1"),
            "modmenu-*.jar"
        );
        assert_eq!(
            asset_pattern("iris-1.7.3+mc1.21.jar", "1.7.3+1.21", "1.21"),
            "iris-*+mc1.21.jar"
        );
        assert_eq!(
            asset_pattern("create-1.21.1-6.0.10.jar", "mc1.21.1-6.0.10", "1.21.1"),
            "create-1.21.1-*.jar"
        );
        assert_eq!(asset_pattern("tool.jar", "nightly", "1.21.1"), "tool.jar");

        assert!(glob_match("create-1.21.1-*.jar", "create-1.21.1-6.1.0.jar"));
        assert!(!glob_match(
            "create-1.21.1-*.jar",
            "create-1.20.1-6.1.0.jar"
        ));
        assert!(glob_match("a*b*c", "abc"));
        assert!(!glob_match("ab*ba", "aba"));
    }

    #[test]
    fn picks_the_loader_build_and_skips_sources() {
        let pack = pack();
        let r = release(
            "1.0",
            &[
                "mod-fabric-1.0.jar",
                "mod-neoforge-1.0.jar",
                "mod-neoforge-1.0-sources.jar",
                "checksums.txt",
            ],
        );
        let picked = pick_asset("o/r", &r, None, &pack).unwrap().unwrap();
        assert_eq!(picked.name, "mod-neoforge-1.0.jar");

        let forge = release("1.0", &["mod-forge-1.0.jar", "mod-neoforge-1.0.jar"]);
        let mut forge_pack = pack.clone();
        forge_pack.loader.kind = LoaderKind::Forge;
        let picked = pick_asset("o/r", &forge, None, &forge_pack)
            .unwrap()
            .unwrap();
        assert_eq!(picked.name, "mod-forge-1.0.jar");

        let unclear = release("1.0", &["mod-a.jar", "mod-b.jar"]);
        assert!(matches!(
            pick_asset("o/r", &unclear, None, &pack),
            Err(ResolveError::AmbiguousAsset { .. })
        ));
        let picked = pick_asset("o/r", &unclear, Some("mod-b*.jar"), &pack).unwrap();
        assert_eq!(picked.unwrap().name, "mod-b.jar");
        assert!(
            pick_asset("o/r", &release("2.0", &[]), None, &pack)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn detects_kind_from_layout() {
        let rp = jar(&[("pack.mcmeta", b"{}"), ("assets/x/a.png", b"")]);
        let dp = jar(&[("pack.mcmeta", b"{}"), ("data/x/a.json", b"")]);
        let sp = jar(&[("shaders/composite.fsh", b"")]);
        assert_eq!(detect_kind("a.zip", &rp), Some(Kind::ResourcePack));
        assert_eq!(detect_kind("a.zip", &dp), Some(Kind::DataPack));
        assert_eq!(detect_kind("a.zip", &sp), Some(Kind::ShaderPack));
        assert_eq!(detect_kind("a.jar", b"not a zip"), Some(Kind::Mod));
        assert_eq!(detect_kind("notes.zip", &jar(&[("x.txt", b"")])), None);
        assert_eq!(detect_kind("a.exe", b""), None);
    }

    struct Fake;

    impl Downloader for Fake {
        async fn download(&self, url: &str) -> Result<Downloaded, String> {
            let minecraft = if url.contains("-2.") {
                "[1.21.4]"
            } else {
                "[1.21.1]"
            };
            let deps = format!(
                "[[dependencies.cool]]\nmodId = \"minecraft\"\ntype = \"required\"\nversionRange = \"{minecraft}\"\n"
            );
            Ok(Downloaded {
                bytes: neoforge_jar("cool_mod", "1.0", &deps.replace("cool]", "cool_mod]")),
                hashes: Hashes {
                    sha512: Some("a".repeat(128)),
                    ..Hashes::default()
                },
                size: 1,
            })
        }
    }

    #[tokio::test]
    async fn newest_release_for_another_minecraft_is_skipped() {
        let pack = pack();
        let releases = [
            release("v2.0", &["cool-2.0.jar"]),
            release("v1.5", &["cool-1.5.jar"]),
        ];
        let pick = pick_release(&Fake, &pack, "o/cool", &releases, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pick.tag, "v1.5");
        assert_eq!(pick.pattern, "cool-*.jar");

        let source = EntrySource::GitHub {
            repo: "o/cool".into(),
            tag: pick.tag,
            asset: pick.pattern,
        };
        let entry = direct_entry(&pack, source, &pick.asset, &pick.file, Some(pick.url)).unwrap();
        assert_eq!(entry.id, "cool-mod");
        assert_eq!(entry.kind, Kind::Mod);
        assert_eq!(entry.file.path.as_str(), "mods/cool-1.5.jar");
    }
}
