use std::path::PathBuf;

use clap::CommandFactory as _;
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use riven_build::workspace::Workspace;

use super::Cli;

/// Answers the shell when it asks for completions (`COMPLETE=<shell>`); returns otherwise.
pub fn answer() {
    // A bare name, so the registration finds `riven` on PATH, `riven.com` on Windows included.
    clap_complete::CompleteEnv::with_factory(Cli::command)
        .bin("riven")
        .completer("riven")
        .complete();
}

/// Ids of the entries in the `riven.json` around the current directory.
pub fn entries() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        Workspace::find()
            .map(|ws| {
                ws.project
                    .content
                    .into_iter()
                    .map(|e| CompletionCandidate::new(e.id).help(Some(e.name.into())))
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn group_candidates() -> Vec<CompletionCandidate> {
    Workspace::find()
        .map(|ws| {
            ws.project
                .groups
                .into_iter()
                .map(|g| CompletionCandidate::new(g.id).help(Some(g.name.into())))
                .collect()
        })
        .unwrap_or_default()
}

/// Ids of the pack's optional groups.
pub fn groups() -> ArgValueCandidates {
    ArgValueCandidates::new(group_candidates)
}

/// The groups, and `none` to take an entry out of its group.
pub fn groups_or_none() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        let mut all = group_candidates();
        all.push(CompletionCandidate::new("none").help(Some("no group".into())));
        all
    })
}

const BEGIN: &str = "# >>> riven completion >>>";
const END: &str = "# <<< riven completion <<<";
const HOOK: &str = "if (Get-Command riven.com -ErrorAction SilentlyContinue) { $env:COMPLETE = 'powershell'; riven.com | Out-String | Invoke-Expression; Remove-Item Env:\\COMPLETE }";

/// The PowerShell profiles of Windows PowerShell 5.1 and PowerShell 7.
fn profiles() -> Vec<PathBuf> {
    let Some(docs) = dirs::document_dir() else {
        return Vec::new();
    };
    ["WindowsPowerShell", "PowerShell"]
        .iter()
        .map(|dir| docs.join(dir).join("Microsoft.PowerShell_profile.ps1"))
        .collect()
}

/// Adds or removes the completion hook in the PowerShell profiles; the Windows installer calls it.
pub fn powershell_profile(remove: bool) -> anyhow::Result<()> {
    for path in profiles() {
        let old = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && remove => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(anyhow::anyhow!("{}: {e}", path.display())),
        };
        let new = if remove {
            without_hook(&old)
        } else {
            with_hook(&old)
        };
        if new != old {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, new).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

const UTF16_BOM: [u8; 2] = [0xFF, 0xFE];

/// Profiles come as UTF-8, a legacy code page or UTF-16 with a BOM; the hook is ASCII, so only
/// UTF-16 needs decoding and the rest is edited as bytes.
fn edit(bytes: &[u8], change: impl Fn(&str) -> String) -> Vec<u8> {
    if let Some(body) = bytes.strip_prefix(&UTF16_BOM) {
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let text = change(&String::from_utf16_lossy(&units));
        let mut out = UTF16_BOM.to_vec();
        out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        return out;
    }
    // Latin-1 maps every byte to one char and back, so other bytes pass through untouched.
    let text: String = bytes.iter().map(|&b| b as char).collect();
    change(&text).chars().map(|c| c as u8).collect()
}

fn with_hook(bytes: &[u8]) -> Vec<u8> {
    edit(bytes, |text| {
        if text.contains(BEGIN) {
            return text.to_owned();
        }
        let newline = if text.contains("\r\n") || text.is_empty() {
            "\r\n"
        } else {
            "\n"
        };
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(newline);
        }
        out.push_str(&format!("{BEGIN}{newline}{HOOK}{newline}{END}{newline}"));
        out
    })
}

fn without_hook(bytes: &[u8]) -> Vec<u8> {
    edit(bytes, |text| {
        let (Some(start), Some(end)) = (text.find(BEGIN), text.find(END)) else {
            return text.to_owned();
        };
        let mut stop = end + END.len();
        for eol in ["\r\n", "\n"] {
            if text[stop..].starts_with(eol) {
                stop += eol.len();
                break;
            }
        }
        format!("{}{}", &text[..start], &text[stop..])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_hook_keeps_the_encoding_and_the_rest_of_the_file() {
        let utf8 = "Set-Alias g git\r\n# Привет\r\n".as_bytes().to_vec();
        let added = with_hook(&utf8);
        assert!(added.starts_with(&utf8));
        assert_eq!(with_hook(&added), added);
        assert_eq!(without_hook(&added), utf8);

        let unix = b"Set-Alias g git".to_vec();
        let added = with_hook(&unix);
        assert!(added.starts_with(b"Set-Alias g git\n# >>> riven"));
        assert_eq!(without_hook(&added), b"Set-Alias g git\n");

        let legacy = vec![b'#', b' ', 0xCF, 0xF0, b'\r', b'\n'];
        assert_eq!(without_hook(&with_hook(&legacy)), legacy);

        let mut utf16 = UTF16_BOM.to_vec();
        utf16.extend("# Привет\r\n".encode_utf16().flat_map(u16::to_le_bytes));
        let added = with_hook(&utf16);
        assert!(added.starts_with(&utf16));
        assert_eq!(without_hook(&added), utf16);

        assert_eq!(without_hook(&with_hook(b"")), b"");
    }
}
