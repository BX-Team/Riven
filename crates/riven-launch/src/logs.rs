use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{LaunchError, io};

const PASTES_API: &str = "https://api.pastes.dev/post";
const PASTES_VIEW: &str = "https://pastes.dev";
const USER_AGENT: &str = "Riven Launcher (github.com/BX-Team/Riven)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Log,
    Crash,
}

/// A log or crash report a game left in its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFile {
    pub path: PathBuf,
    pub name: String,
    pub kind: LogKind,
    pub modified: SystemTime,
}

/// `logs/*.log[.gz]` and `crash-reports/*.txt`, newest first.
pub fn list(game_dir: &Path) -> Vec<LogFile> {
    let mut out = Vec::new();
    for (folder, kind) in [("logs", LogKind::Log), ("crash-reports", LogKind::Crash)] {
        let Ok(entries) = std::fs::read_dir(game_dir.join(folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let wanted = match kind {
                LogKind::Log => name.ends_with(".log") || name.ends_with(".log.gz"),
                LogKind::Crash => name.ends_with(".txt"),
            };
            let Ok(meta) = entry.metadata() else { continue };
            if !wanted || !meta.is_file() {
                continue;
            }
            out.push(LogFile {
                path: entry.path(),
                name,
                kind,
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.modified));
    out
}

/// A log's text, unpacked when it is gzipped.
pub fn read(path: &Path) -> Result<String, LaunchError> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    let bytes = if path.extension().is_some_and(|e| e == "gz") {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .read_to_end(&mut out)
            .map_err(io(path))?;
        out
    } else {
        bytes
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Hides what could sign in as the player: the value after `--accessToken`, and JWTs.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("--accessToken") {
        let (head, tail) = rest.split_at(at + "--accessToken".len());
        out.push_str(head);
        let gap = tail.len() - tail.trim_start_matches([' ', ',', '=', '\t']).len();
        out.push_str(&tail[..gap]);
        let value = &tail[gap..];
        let end = value
            .find(|c: char| c.is_whitespace() || c == ',' || c == ']')
            .unwrap_or(value.len());
        if end > 0 {
            out.push_str("<redacted>");
        }
        rest = &value[end..];
    }
    out.push_str(rest);
    redact_jwts(&out)
}

fn redact_jwts(text: &str) -> String {
    let token_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("eyJ") {
        out.push_str(&rest[..at]);
        let candidate = &rest[at..];
        let end = candidate
            .find(|c| !token_char(c))
            .unwrap_or(candidate.len());
        let token = &candidate[..end];
        if token.matches('.').count() == 2 && token.len() > 40 {
            out.push_str("<redacted>");
        } else {
            out.push_str(token);
        }
        rest = &candidate[end..];
    }
    out.push_str(rest);
    out
}

/// Uploads text to pastes.dev with tokens hidden and returns the paste's link.
pub async fn upload(text: &str) -> Result<String, LaunchError> {
    #[derive(serde::Deserialize)]
    struct Posted {
        key: String,
    }
    let failed = |e: reqwest::Error| LaunchError::Upload(e.to_string());
    let response = riven_sources::client()
        .post(PASTES_API)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::CONTENT_TYPE, "text/log")
        .body(redact(text))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?;
    let posted: Posted = response.json().await.map_err(failed)?;
    Ok(format!("{PASTES_VIEW}/{}", posted.key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_never_carry_tokens() {
        let jwt = format!("eyJhbGciOiJIUzI1NiJ9.{}.sig_nature-x", "a".repeat(40));
        let text = format!(
            "args: [--username, Steve, --accessToken, {jwt}, --version, 1.21.1]\n\
             cmd --accessToken abc123 --uuid x\nbearer {jwt} end\neyJshort.a.b stays"
        );
        let clean = redact(&text);
        assert!(!clean.contains("abc123"));
        assert!(!clean.contains(&jwt));
        assert!(clean.contains("--accessToken, <redacted>, --version"));
        assert!(clean.contains("--accessToken <redacted> --uuid"));
        assert!(clean.contains("bearer <redacted> end"));
        assert!(clean.contains("eyJshort.a.b stays"));
    }
}
