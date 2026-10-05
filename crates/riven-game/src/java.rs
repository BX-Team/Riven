use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::net::{Checksum, Download};
use crate::{Dirs, Error, Event, Report, Step, io, net, os};

const ADOPTIUM: &str = "https://api.adoptium.net/v3/assets/latest";

#[derive(Deserialize)]
struct Release {
    binary: Binary,
}

#[derive(Deserialize)]
struct Binary {
    package: Package,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    link: String,
    checksum: String,
    size: u64,
}

/// What Adoptium calls this CPU, or x86-64 for games that need Rosetta or Windows' emulation.
fn arch(x64: bool) -> &'static str {
    if x64 || cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "x86-32"
    } else {
        "arm"
    }
}

fn adoptium_os() -> &'static str {
    match os::NAME {
        "osx" => "mac",
        name => name,
    }
}

/// Temurin `java` for `major`, downloaded on first use; 16 runs on 17, CPUs without a build use x86-64.
pub async fn ensure(
    http: &reqwest::Client,
    dirs: &Dirs,
    major: u32,
    x64: bool,
    report: Report<'_>,
) -> Result<PathBuf, Error> {
    let major = if major == 16 { 17 } else { major };
    let mut archs = vec![arch(x64)];
    if archs[0] != "x64" && !cfg!(target_os = "linux") {
        archs.push("x64");
    }
    for arch in &archs {
        if let Some(java) = find(&home(dirs, major, arch)) {
            return Ok(java);
        }
    }
    for arch in archs {
        let url = format!(
            "{ADOPTIUM}/{major}/hotspot?os={}&architecture={arch}&image_type=jre&vendor=eclipse",
            adoptium_os()
        );
        let releases: Vec<Release> = net::get_json(http, &url).await?;
        let Some(release) = releases.into_iter().next() else {
            continue;
        };
        report(Event::Step(Step::Java));
        return install(http, dirs, major, arch, release.binary.package, report).await;
    }
    Err(Error::NoJava(major))
}

fn home(dirs: &Dirs, major: u32, arch: &str) -> PathBuf {
    dirs.java().join(format!("temurin-{major}-{arch}"))
}

async fn install(
    http: &reqwest::Client,
    dirs: &Dirs,
    major: u32,
    arch: &str,
    package: Package,
    report: Report<'_>,
) -> Result<PathBuf, Error> {
    let archive = Download {
        urls: vec![package.link],
        path: dirs.java().join(&package.name),
        checksum: Some(Checksum::Sha256(package.checksum)),
        size: Some(package.size),
    };
    net::fetch_all(http, std::slice::from_ref(&archive), report).await?;
    let home = home(dirs, major, arch);
    let archive_path = archive.path.clone();
    let java = crate::blocking(move || {
        let unpacked = home.with_extension("part");
        let _ = std::fs::remove_dir_all(&unpacked);
        unpack(&archive_path, &unpacked)?;
        let _ = std::fs::remove_dir_all(&home);
        std::fs::rename(&unpacked, &home).map_err(io(&home))?;
        let _ = std::fs::remove_file(&archive_path);
        find(&home).ok_or_else(|| {
            Error::Missing(
                format!("java in {}", home.display()),
                "the Temurin archive has no bin/java".into(),
            )
        })
    })
    .await?;
    Ok(java)
}

fn unpack(archive: &Path, target: &Path) -> Result<(), Error> {
    let file = std::fs::File::open(archive).map_err(io(archive))?;
    if archive.extension().is_some_and(|e| e == "zip") {
        let mut zip = zip::ZipArchive::new(file).map_err(|source| Error::Zip {
            path: archive.to_owned(),
            source,
        })?;
        zip.extract(target).map_err(|source| Error::Zip {
            path: archive.to_owned(),
            source,
        })
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(target)
            .map_err(io(archive))
    }
}

/// `bin/java` in the one folder an unpacked JRE archive holds; macOS bundles keep it in `Contents/Home`.
fn find(home: &Path) -> Option<PathBuf> {
    let root = std::fs::read_dir(home)
        .ok()?
        .flatten()
        .find(|e| e.path().is_dir())?
        .path();
    [root.join("Contents/Home/bin"), root.join("bin")]
        .into_iter()
        .map(|bin| bin.join(os::java_exe()))
        .find(|java| java.is_file())
}
