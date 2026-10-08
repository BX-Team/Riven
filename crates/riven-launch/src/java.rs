use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Installed memory in MiB, where the system says; Linux reads `/proc/meminfo`.
pub fn total_memory_mb() -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = text
            .lines()
            .find_map(|l| l.strip_prefix("MemTotal:"))?
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .ok()?;
        u32::try_from(kb / 1024).ok()
    }
    #[cfg(not(target_os = "linux"))]
    None
}

/// A Java runtime found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaInstall {
    pub path: PathBuf,
    /// `java.version`, such as `21.0.5` or `1.8.0_402`.
    pub version: String,
    pub major: u32,
    pub vendor: String,
    pub arch: String,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

fn exe() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}

/// `1.8.0_402` → 8, `21.0.5` → 21, `17` → 17.
pub fn major_of(version: &str) -> Option<u32> {
    let mut parts = version.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// The Java major a Minecraft release needs; snapshots and unknown names give none.
pub fn required_major(minecraft: &str) -> Option<u32> {
    let mut parts = minecraft.split('.').map(|p| p.parse::<u32>().ok());
    let (Some(Some(first)), Some(Some(minor))) = (parts.next(), parts.next()) else {
        return None;
    };
    let patch = parts.next().flatten().unwrap_or(0);
    Some(match (first, minor, patch) {
        (1, 0..=16, _) => 8,
        (1, 17, _) => 16,
        (1, 18..=19, _) | (1, 20, 0..=4) => 17,
        (1, _, _) => 21,
        _ => 25,
    })
}

/// Reads the properties `java -XshowSettings:properties -version` prints to stderr.
fn parse(output: &str) -> Option<(String, String, String)> {
    let mut version = None;
    let mut vendor = None;
    let mut arch = None;
    for line in output.lines() {
        let Some((key, value)) = line.trim().split_once(" = ") else {
            continue;
        };
        let value = value.trim().to_owned();
        match key {
            "java.version" => version = Some(value),
            "java.vendor" => vendor = Some(value),
            "os.arch" => arch = Some(value),
            _ => {}
        }
    }
    Some((
        version?,
        vendor.unwrap_or_default(),
        arch.unwrap_or_default(),
    ))
}

/// Runs `java` once to learn its version; hangs and crashes count as not Java.
pub fn probe(path: &Path) -> Option<JavaInstall> {
    let mut cmd = Command::new(path);
    cmd.args(["-XshowSettings:properties", "-version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stderr = child.stderr.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
        text
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let (version, vendor, arch) = parse(&reader.join().ok()?)?;
    Some(JavaInstall {
        path: path.to_owned(),
        major: major_of(&version)?,
        version,
        vendor,
        arch,
    })
}

/// Every path matching `pattern`, where a `*` component stands for any directory entry.
fn expand(pattern: &Path) -> Vec<PathBuf> {
    let mut found = vec![PathBuf::new()];
    for part in pattern.components() {
        let part = part.as_os_str();
        found = if part == "*" {
            found
                .iter()
                .filter_map(|dir| std::fs::read_dir(dir).ok())
                .flat_map(|entries| entries.flatten().map(|e| e.path()))
                .collect()
        } else {
            found.into_iter().map(|p| p.join(part)).collect()
        };
    }
    found.into_iter().filter(|p| p.is_file()).collect()
}

/// Folders Java is commonly installed into, as `*` patterns ending in the executable.
fn patterns() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let bin = |root: PathBuf| root.join("bin").join(exe());
    let mut out = Vec::new();
    if let Some(data) = riven_sync::data_dir() {
        out.push(bin(data.join("java").join("*").join("*")));
        out.push(data.join("java/*/*/Contents/Home/bin").join(exe()));
    }
    if cfg!(target_os = "linux") {
        for root in [
            "/usr/lib/jvm/*",
            "/usr/lib64/jvm/*",
            "/usr/lib32/jvm/*",
            "/usr/java/*",
            "/opt/*",
            "/opt/java/*",
            "/opt/jdk/*",
            "/run/current-system/sw",
        ] {
            out.push(bin(PathBuf::from(root)));
        }
        for root in [
            ".nix-profile",
            ".sdkman/candidates/java/*",
            ".jdks/*",
            ".local/share/PrismLauncher/java/*",
            ".local/share/PrismLauncher/java/*/*",
            ".minecraft/runtime/*/*/*",
        ] {
            out.push(bin(home.join(root)));
        }
    } else if cfg!(target_os = "macos") {
        for root in [
            PathBuf::from("/Library/Java/JavaVirtualMachines/*/Contents/Home"),
            home.join("Library/Java/JavaVirtualMachines/*/Contents/Home"),
            PathBuf::from("/opt/homebrew/opt/*"),
            PathBuf::from("/usr/local/opt/*"),
            home.join(".sdkman/candidates/java/*"),
            home.join(".jdks/*/Contents/Home"),
            home.join("Library/Application Support/PrismLauncher/java/*"),
            home.join(
                "Library/Application Support/minecraft/runtime/*/*/*/jre.bundle/Contents/Home",
            ),
        ] {
            out.push(bin(root));
        }
    } else if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
            let Some(base) = std::env::var_os(var).map(PathBuf::from) else {
                continue;
            };
            for vendor in [
                "Java",
                "Eclipse Adoptium",
                "Eclipse Foundation",
                "AdoptOpenJDK",
                "Zulu",
                "Microsoft",
                "BellSoft",
                "Amazon Corretto",
                "Semeru",
                "Oracle",
            ] {
                out.push(bin(base.join(vendor).join("*")));
            }
        }
        out.push(bin(home.join(".jdks").join("*")));
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            out.push(bin(appdata.join(".minecraft/runtime/*/*/*")));
            out.push(bin(appdata.join("PrismLauncher/java/*")));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            out.push(bin(local.join(
                "Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/*/*/*",
            )));
        }
    }
    out
}

/// Java executables worth probing: `JAVA_HOME`, `PATH` and the usual install folders.
pub fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        paths.push(PathBuf::from(home).join("bin").join(exe()));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            if cfg!(target_os = "macos") && dir == Path::new("/usr/bin") {
                continue;
            }
            paths.push(dir.join(exe()));
        }
    }
    for pattern in patterns() {
        paths.extend(expand(&pattern));
    }
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|p| p.is_file())
        .filter_map(|p| std::fs::canonicalize(&p).ok())
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Finds and probes every Java on this machine, newest major first.
pub fn detect() -> Vec<JavaInstall> {
    let mut found: Vec<JavaInstall> = std::thread::scope(|scope| {
        let probes: Vec<_> = candidates()
            .into_iter()
            .map(|path| scope.spawn(move || probe(&path)))
            .collect();
        probes
            .into_iter()
            .filter_map(|p| p.join().ok().flatten())
            .collect()
    });
    found.sort_by(|a, b| b.major.cmp(&a.major).then_with(|| a.path.cmp(&b.path)));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn majors_follow_both_version_schemes() {
        assert_eq!(major_of("1.8.0_402"), Some(8));
        assert_eq!(major_of("21.0.5"), Some(21));
        assert_eq!(major_of("17"), Some(17));
        assert_eq!(major_of("25-ea"), Some(25));
        assert_eq!(major_of("abc"), None);
    }

    #[test]
    fn minecraft_versions_map_to_their_java() {
        assert_eq!(required_major("1.12.2"), Some(8));
        assert_eq!(required_major("1.17.1"), Some(16));
        assert_eq!(required_major("1.20.4"), Some(17));
        assert_eq!(required_major("1.20.5"), Some(21));
        assert_eq!(required_major("1.21"), Some(21));
        assert_eq!(required_major("26.3"), Some(25));
        assert_eq!(required_major("24w14a"), None);
    }

    #[test]
    fn reads_properties_among_other_output() {
        let output = "Property settings:\n    file.encoding = UTF-8\n    java.home = /usr/lib/jvm/java-21\n    java.vendor = Eclipse Adoptium\n    java.version = 21.0.5\n    os.arch = amd64\n    sun.boot.library.path = /x\n\nopenjdk version \"21.0.5\" 2024-10-15 LTS\n";
        assert_eq!(
            parse(output),
            Some(("21.0.5".into(), "Eclipse Adoptium".into(), "amd64".into()))
        );
        assert_eq!(parse("Error: could not create the Java VM"), None);
    }
}
