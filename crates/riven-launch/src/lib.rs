pub mod accounts;
pub mod game;
pub mod instances;
pub mod java;
pub mod mods;

use std::path::{Path, PathBuf};

use riven_format::{Document, Settings};

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("cannot locate the {0} directory")]
    NoDir(&'static str),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Format {
        path: PathBuf,
        source: riven_format::Error,
    },
    #[error("no instance `{0}`")]
    NoInstance(String),
    #[error("`{0}` is not a valid player name (3–16 letters, digits or _)")]
    BadName(String),
    #[error("{} is not a Java executable", .0.display())]
    BadJava(PathBuf),
    #[error("`{command}` failed with code {code:?}")]
    Hook { command: String, code: Option<i32> },
    #[error("Microsoft sign-in is waiting for Mojang's approval; play with an offline account")]
    MicrosoftPending,
    #[error("cannot start the game: {0}")]
    Game(String),
    #[error(transparent)]
    Sync(#[from] riven_sync::SyncError),
}

/// Links a folder to another: a symlink, or on Windows a junction, which needs no admin rights.
pub(crate) fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return junction::create(target, link);
}

/// Removes a link made by [`link_dir`], leaving its target alone.
pub(crate) fn unlink_dir(link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::fs::remove_file(link);
    #[cfg(windows)]
    return std::fs::remove_dir(link);
}

/// Drops ANSI escape sequences (colors, cursor moves) that games print to terminals.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if !c.is_control() || c == '\t' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The first 16 hex digits of a SHA-256, for folder names derived from paths.
pub(crate) fn short_hash(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))[..16].to_owned()
}

/// The current UTC time as RFC 3339, `2026-10-07T14:03:00Z`.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

/// Seconds since the Unix epoch for `2026-10-07T14:03:00Z`; offsets other than `Z` are ignored.
pub fn parse_rfc3339(text: &str) -> Option<u64> {
    let num = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second).ok()
}

fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> LaunchError + '_ {
    move |source| LaunchError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Reads a document, or its default when the file does not exist yet.
pub(crate) fn load_or_default<T: Document + Default>(path: &Path) -> Result<T, LaunchError> {
    match std::fs::read_to_string(path) {
        Ok(text) => riven_format::from_str(&text).map_err(|source| LaunchError::Format {
            path: path.to_owned(),
            source,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(io(path)(e)),
    }
}

/// Writes a document through a temporary file so a crash never leaves half of it.
pub(crate) fn save<T: Document>(path: &Path, doc: &T) -> Result<(), LaunchError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io(parent))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, riven_format::to_string(doc)).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
}

fn config_file(name: &str) -> Result<PathBuf, LaunchError> {
    Ok(riven_sync::config_dir()
        .ok_or(LaunchError::NoDir("config"))?
        .join(name))
}

pub fn settings_path() -> Result<PathBuf, LaunchError> {
    config_file("settings.json")
}

pub fn load_settings() -> Result<Settings, LaunchError> {
    load_or_default(&settings_path()?)
}

pub fn save_settings(settings: &Settings) -> Result<(), LaunchError> {
    save(&settings_path()?, settings)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_handles_leap_days_and_epoch() {
        assert_eq!(super::rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(super::rfc3339(1_791_381_780), "2026-10-07T14:03:00Z");
    }

    #[test]
    fn rfc3339_parses_back() {
        for secs in [0, 951_782_400, 1_791_381_780, 4_102_444_799] {
            assert_eq!(super::parse_rfc3339(&super::rfc3339(secs)), Some(secs));
        }
        assert_eq!(super::parse_rfc3339("not a date"), None);
    }
}
