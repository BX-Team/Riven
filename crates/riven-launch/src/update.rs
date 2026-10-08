use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::LaunchError;

const LATEST: &str = "https://api.github.com/repos/BX-Team/Riven/releases/latest";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const BUNDLE: &str = "Riven Launcher.app";
/// Set on the updated process, which then waits for this one to let go of what it held.
pub const RELAUNCH_ENV: &str = "RIVEN_RELAUNCH";

/// Which binary is running: the full launcher or the headless `riven-cli` build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edition {
    Launcher,
    Cli,
}

/// How this copy of Riven was installed, which decides how it updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// A Nix store path: updated through the flake.
    Nix,
    /// A deb, rpm or Arch package: updated by the system package manager.
    System,
    AppImage(PathBuf),
    /// The Windows installer's copy, updated by running the new installer silently.
    Installer(PathBuf),
    /// The `.app` bundle on macOS.
    Bundle(PathBuf),
    /// A binary unpacked from an archive.
    Portable(PathBuf),
}

impl Install {
    pub fn detect() -> Self {
        let exe = std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .unwrap_or_default();
        let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
        classify(std::env::consts::OS, &exe, appimage, |p| p.exists())
    }

    /// Whether Riven can replace itself here, rather than a package manager.
    pub fn self_updates(&self) -> bool {
        !matches!(self, Install::Nix | Install::System)
    }

    fn asset(&self, edition: Edition) -> Option<String> {
        let arch = std::env::consts::ARCH;
        let cli = edition == Edition::Cli;
        Some(match self {
            Install::Nix | Install::System => return None,
            Install::AppImage(_) => format!("Riven-{arch}.AppImage"),
            Install::Installer(_) => format!("Riven-Setup-{arch}.exe"),
            Install::Bundle(_) => "Riven-macos.app.tar.gz".into(),
            Install::Portable(_) => {
                let name = if cli { "riven-cli" } else { "riven" };
                match std::env::consts::OS {
                    "windows" => format!("{name}-{arch}-windows.zip"),
                    "macos" => format!("{name}-macos.tar.gz"),
                    os => format!("{name}-{arch}-{os}.tar.gz"),
                }
            }
        })
    }
}

fn classify(
    os: &str,
    exe: &Path,
    appimage: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Install {
    if exe.starts_with("/nix/store") {
        return Install::Nix;
    }
    match os {
        "linux" => match appimage {
            Some(file) => Install::AppImage(file),
            None if exe.starts_with("/usr") && !exe.starts_with("/usr/local") => Install::System,
            None => Install::Portable(exe.to_owned()),
        },
        "windows"
            if exe
                .parent()
                .is_some_and(|d| exists(&d.join("uninstall.exe"))) =>
        {
            Install::Installer(exe.to_owned())
        }
        "macos" => match exe.ancestors().nth(3) {
            Some(app) if app.extension().is_some_and(|e| e == "app") => {
                Install::Bundle(app.to_owned())
            }
            _ => Install::Portable(exe.to_owned()),
        },
        _ => Install::Portable(exe.to_owned()),
    }
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    /// The release notes, Markdown.
    pub notes: String,
    /// The release page on GitHub.
    pub page: String,
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    /// `sha256:<hex>`, filled in by GitHub for every uploaded asset.
    digest: Option<String>,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    assets: Vec<Asset>,
}

fn failed(e: impl std::fmt::Display) -> LaunchError {
    LaunchError::Update(e.to_string())
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> LaunchError + '_ {
    move |source| LaunchError::Io {
        path: path.to_owned(),
        source,
    }
}

/// `true` when `version` is a later release than the running one.
pub fn is_newer(version: &str) -> bool {
    let parse = |v: &str| semver::Version::parse(v.trim_start_matches('v')).ok();
    match (parse(version), parse(CURRENT)) {
        (Some(new), Some(current)) => new > current,
        _ => false,
    }
}

/// The newest stable release on GitHub, if there is one yet.
pub async fn latest() -> Result<Option<Release>, LaunchError> {
    let response = riven_sources::client()
        .get(LATEST)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(failed)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let release: GithubRelease = response
        .error_for_status()
        .map_err(failed)?
        .json()
        .await
        .map_err(failed)?;
    Ok(Some(Release {
        version: release.tag_name.trim_start_matches('v').to_owned(),
        notes: release.body.unwrap_or_default(),
        page: release.html_url,
        assets: release.assets,
    }))
}

/// The newest release when it is newer than this build.
pub async fn check() -> Result<Option<Release>, LaunchError> {
    Ok(latest().await?.filter(|r| is_newer(&r.version)))
}

/// What starts the updated Riven once this process is gone.
#[derive(Debug)]
pub struct Relaunch(PathBuf);

impl Relaunch {
    pub fn spawn(self) -> std::io::Result<()> {
        if cfg!(target_os = "macos") && self.0.extension().is_some_and(|e| e == "app") {
            Command::new("open")
                .args(["-n", "--env", &format!("{RELAUNCH_ENV}=1")])
                .arg(&self.0)
                .spawn()?;
        } else {
            Command::new(&self.0).env(RELAUNCH_ENV, "1").spawn()?;
        }
        Ok(())
    }
}

/// Downloads `release` for this install and puts it in place; `progress` gets `(done, total)` bytes.
pub async fn install(
    release: &Release,
    install: &Install,
    edition: Edition,
    progress: impl Fn(u64, u64),
) -> Result<Relaunch, LaunchError> {
    let name = install.asset(edition).ok_or(LaunchError::Managed)?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| failed(format!("release {} has no {name}", release.version)))?;
    let staging = staging_dir(install)?;
    let file = staging.join(&asset.name);
    let result = async {
        download(asset, &file, progress).await?;
        let staging = staging.clone();
        let install = install.clone();
        tokio::task::spawn_blocking(move || apply(&install, &file, &staging))
            .await
            .map_err(failed)?
    }
    .await;
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// A scratch folder on the same volume as what gets replaced, so the final moves are renames.
fn staging_dir(install: &Install) -> Result<PathBuf, LaunchError> {
    let target = match install {
        Install::Nix | Install::System => return Err(LaunchError::Managed),
        Install::Installer(_) => return Ok(std::env::temp_dir().join("riven-update")),
        Install::AppImage(p) | Install::Bundle(p) | Install::Portable(p) => p,
    };
    let parent = target.parent().unwrap_or(Path::new("."));
    let dir = parent.join(format!(".riven-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|source| {
        if source.kind() == std::io::ErrorKind::PermissionDenied {
            LaunchError::ReadOnly(parent.to_owned())
        } else {
            LaunchError::Io {
                path: dir.clone(),
                source,
            }
        }
    })?;
    Ok(dir)
}

async fn download(
    asset: &Asset,
    to: &Path,
    progress: impl Fn(u64, u64),
) -> Result<(), LaunchError> {
    let expected = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .ok_or_else(|| failed(format!("{} has no checksum", asset.name)))?
        .to_ascii_lowercase();
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(io_error(dir))?;
    }
    let mut response = riven_sources::client()
        .get(&asset.browser_download_url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?;
    let mut out = std::fs::File::create(to).map_err(io_error(to))?;
    let mut hasher = Sha256::new();
    let mut done = 0;
    progress(0, asset.size);
    while let Some(chunk) = response.chunk().await.map_err(failed)? {
        hasher.update(&chunk);
        out.write_all(&chunk).map_err(io_error(to))?;
        done += chunk.len() as u64;
        progress(done, asset.size);
    }
    out.sync_all().map_err(io_error(to))?;
    verify(&hex::encode(hasher.finalize()), &expected, &asset.name)
}

fn verify(actual: &str, expected: &str, name: &str) -> Result<(), LaunchError> {
    if actual == expected {
        Ok(())
    } else {
        Err(failed(format!(
            "{name} is damaged: sha256 {actual}, expected {expected}"
        )))
    }
}

fn apply(install: &Install, file: &Path, staging: &Path) -> Result<Relaunch, LaunchError> {
    match install {
        Install::Nix | Install::System => Err(LaunchError::Managed),
        Install::AppImage(target) => {
            make_executable(file)?;
            std::fs::rename(file, target).map_err(io_error(target))?;
            Ok(Relaunch(target.clone()))
        }
        Install::Portable(exe) => {
            let name = if cfg!(windows) { "riven.exe" } else { "riven" };
            let binary = staging.join(name);
            extract_file(file, name, &binary)?;
            make_executable(&binary)?;
            self_replace::self_replace(&binary).map_err(io_error(exe))?;
            Ok(Relaunch(exe.clone()))
        }
        Install::Bundle(app) => {
            let unpacked = staging.join("unpacked");
            unpack_tar(file, &unpacked)?;
            let fresh = unpacked.join(BUNDLE);
            if !fresh.is_dir() {
                return Err(failed(format!("the update has no {BUNDLE}")));
            }
            let old = staging.join("old.app");
            std::fs::rename(app, &old).map_err(io_error(app))?;
            if let Err(source) = std::fs::rename(&fresh, app) {
                let _ = std::fs::rename(&old, app);
                return Err(LaunchError::Io {
                    path: app.clone(),
                    source,
                });
            }
            Ok(Relaunch(app.clone()))
        }
        Install::Installer(exe) => run_installer(file, exe),
    }
}

/// A running exe cannot be overwritten on Windows but can be renamed, so the installer finds its place free.
fn run_installer(setup: &Path, exe: &Path) -> Result<Relaunch, LaunchError> {
    let aside = exe.with_extension("exe.old");
    let _ = std::fs::remove_file(&aside);
    std::fs::rename(exe, &aside).map_err(io_error(exe))?;
    let status = Command::new(setup).arg("/S").status();
    match status {
        Ok(s) if s.success() && exe.exists() => Ok(Relaunch(exe.to_owned())),
        outcome => {
            let _ = std::fs::rename(&aside, exe);
            Err(failed(match outcome {
                Ok(s) => format!("the installer exited with {s}"),
                Err(e) => format!("cannot run the installer: {e}"),
            }))
        }
    }
}

/// Removes what an earlier update left behind; call once at startup.
pub fn cleanup() {
    if !cfg!(windows) {
        return;
    }
    if let Install::Installer(exe) = Install::detect() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
}

fn extract_file(archive: &Path, name: &str, to: &Path) -> Result<(), LaunchError> {
    let file = std::fs::File::open(archive).map_err(io_error(archive))?;
    let mut bytes = Vec::new();
    if archive.extension().is_some_and(|e| e == "zip") {
        let mut zip = zip::ZipArchive::new(file).map_err(failed)?;
        zip.by_name(name)
            .map_err(failed)?
            .read_to_end(&mut bytes)
            .map_err(io_error(archive))?;
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        let mut entry = tar
            .entries()
            .map_err(io_error(archive))?
            .filter_map(Result::ok)
            .find(|e| e.path().is_ok_and(|p| p == Path::new(name)))
            .ok_or_else(|| failed(format!("the update has no {name}")))?;
        entry.read_to_end(&mut bytes).map_err(io_error(archive))?;
    }
    std::fs::write(to, bytes).map_err(io_error(to))
}

fn unpack_tar(archive: &Path, to: &Path) -> Result<(), LaunchError> {
    let file = std::fs::File::open(archive).map_err(io_error(archive))?;
    std::fs::create_dir_all(to).map_err(io_error(to))?;
    tar::Archive::new(flate2::read::GzDecoder::new(file))
        .unpack(to)
        .map_err(io_error(archive))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), LaunchError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(io_error(path))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<(), LaunchError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_kind_follows_where_the_binary_lives() {
        let none = |_: &Path| false;
        let p = Path::new;
        assert_eq!(
            classify("linux", p("/nix/store/abc-riven/bin/riven"), None, none),
            Install::Nix
        );
        assert_eq!(
            classify("linux", p("/usr/bin/riven"), None, none),
            Install::System
        );
        assert_eq!(
            classify("linux", p("/usr/local/bin/riven"), None, none),
            Install::Portable("/usr/local/bin/riven".into())
        );
        assert_eq!(
            classify(
                "linux",
                p("/tmp/.mount_Riven/usr/bin/riven"),
                Some("/home/a/Riven.AppImage".into()),
                none
            ),
            Install::AppImage("/home/a/Riven.AppImage".into())
        );
        assert_eq!(
            classify(
                "macos",
                p("/Applications/Riven Launcher.app/Contents/MacOS/riven"),
                None,
                none
            ),
            Install::Bundle("/Applications/Riven Launcher.app".into())
        );
        assert_eq!(
            classify("windows", p("C:/Riven/riven.exe"), None, |f| f
                .ends_with("uninstall.exe")),
            Install::Installer("C:/Riven/riven.exe".into())
        );
        assert_eq!(
            classify("windows", p("C:/Riven/riven.exe"), None, none),
            Install::Portable("C:/Riven/riven.exe".into())
        );
    }

    #[test]
    fn only_later_stable_versions_are_updates() {
        assert!(is_newer("v99.0.0"));
        assert!(!is_newer(CURRENT));
        assert!(!is_newer("0.0.1"));
        assert!(!is_newer("nightly"));
        let pre = format!("{CURRENT}-beta.1");
        assert!(!is_newer(&pre));
    }
}
