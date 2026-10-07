use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use gix::bstr::{BString, ByteSlice as _};
use riven_format::{Entry, Project, UpdatePolicy};
use tokio::io::{AsyncRead, AsyncReadExt as _};

use crate::workspace::PROJECT_FILE;

const LOG_LENGTH: usize = 30;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git is not installed or not in PATH")]
    NoGit,
    #[error("cannot read the repository: {0}")]
    Read(String),
    #[error("git could not authenticate; set up an SSH key or run `gh auth login`")]
    Auth,
    #[error("`git {command}` failed: {message}")]
    Failed { command: String, message: String },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

fn read(e: impl Display) -> GitError {
    GitError::Read(e.to_string())
}

/// How a file differs from `HEAD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileState {
    Conflict,
    Modified,
    Added,
    Removed,
    Renamed,
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStatus {
    /// Relative to the project folder.
    pub path: String,
    pub state: FileState,
    /// Whether the change is in the index.
    pub staged: bool,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub id: String,
    pub summary: String,
    pub author: String,
    /// Seconds since the Unix epoch.
    pub time: i64,
}

/// What the repository around a project looks like, read without running git.
#[derive(Debug, Clone)]
pub struct Status {
    /// `None` when `HEAD` is detached.
    pub branch: Option<String>,
    /// The tracking branch, like `origin/master`.
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<FileStatus>,
    pub log: Vec<Commit>,
    /// `riven.json` as committed in `HEAD`, if it is there.
    pub committed: Option<Project>,
}

/// The project's folder relative to the repository root, `""` at the root.
fn prefix(repo: &gix::Repository, dir: &Path) -> Result<String, GitError> {
    let root = repo
        .workdir()
        .ok_or_else(|| GitError::Read("the repository has no working tree".into()))?;
    let canonical = |p: &Path| {
        std::fs::canonicalize(p).map_err(|source| GitError::Io {
            path: p.to_owned(),
            source,
        })
    };
    let (root, dir) = (canonical(root)?, canonical(dir)?);
    let rel = dir.strip_prefix(&root).map_err(read)?;
    Ok(rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn strip<'a>(prefix: &str, path: &'a str) -> Option<&'a str> {
    if prefix.is_empty() {
        return Some(path);
    }
    path.strip_prefix(prefix)?.strip_prefix('/')
}

/// Reads the repository holding `dir`; `None` when it is not in one.
pub fn status(dir: &Path) -> Result<Option<Status>, GitError> {
    let Ok(repo) = gix::discover(dir) else {
        return Ok(None);
    };
    let prefix = prefix(&repo, dir)?;
    let head_name = repo.head_name().map_err(read)?;
    let branch = head_name.as_ref().map(|n| n.shorten().to_string());
    let head = repo.head_commit().ok();

    let tracking = head_name.as_ref().and_then(|name| {
        repo.branch_remote_tracking_ref_name(name.as_ref(), gix::remote::Direction::Fetch)
            .and_then(Result::ok)
    });
    let upstream_id = tracking.as_ref().and_then(|name| {
        repo.find_reference(name.as_ref())
            .ok()?
            .peel_to_id()
            .ok()
            .map(|id| id.detach())
    });
    let upstream = tracking
        .as_ref()
        .filter(|_| upstream_id.is_some())
        .map(|n| n.shorten().to_string());
    let (mut ahead, mut behind) = (0, 0);
    if let (Some(head), Some(up)) = (&head, upstream_id) {
        let count = |from: gix::ObjectId, hide: gix::ObjectId| -> Result<usize, GitError> {
            Ok(repo
                .rev_walk([from])
                .with_hidden([hide])
                .all()
                .map_err(read)?
                .count())
        };
        ahead = count(head.id, up)?;
        behind = count(up, head.id)?;
    }

    let mut log = Vec::new();
    if let Some(head) = &head {
        for info in repo
            .rev_walk([head.id])
            .all()
            .map_err(read)?
            .take(LOG_LENGTH)
        {
            let info = info.map_err(read)?;
            let commit = info.object().map_err(read)?;
            let summary = commit
                .message()
                .map(|m| m.summary().to_string())
                .unwrap_or_default();
            let author = commit
                .author()
                .map(|a| a.name.to_string())
                .unwrap_or_default();
            let time = commit.time().map(|t| t.seconds).unwrap_or_default();
            log.push(Commit {
                id: info.id.to_hex_with_len(7).to_string(),
                summary,
                author,
                time,
            });
        }
    }

    let committed = head.as_ref().and_then(|head| {
        let path = match prefix.as_str() {
            "" => PROJECT_FILE.to_owned(),
            p => format!("{p}/{PROJECT_FILE}"),
        };
        let entry = head.tree().ok()?.lookup_entry_by_path(path).ok()??;
        let blob = entry.object().ok()?;
        riven_format::from_str(std::str::from_utf8(&blob.data).ok()?).ok()
    });

    let patterns: Vec<BString> = match prefix.as_str() {
        "" => Vec::new(),
        p => vec![BString::from(p)],
    };
    let mut files: BTreeMap<String, FileStatus> = BTreeMap::new();
    let items = repo
        .status(gix::progress::Discard)
        .map_err(read)?
        .into_iter(patterns)
        .map_err(read)?;
    for item in items {
        let item = item.map_err(read)?;
        let (state, staged) = match &item {
            gix::status::Item::TreeIndex(change) => {
                use gix::diff::index::ChangeRef;
                let state = match change {
                    ChangeRef::Addition { .. } => FileState::Added,
                    ChangeRef::Deletion { .. } => FileState::Removed,
                    ChangeRef::Modification { .. } => FileState::Modified,
                    ChangeRef::Rewrite { .. } => FileState::Renamed,
                };
                (state, true)
            }
            gix::status::Item::IndexWorktree(change) => {
                use gix::status::index_worktree::iter::Summary;
                let state = match change.summary() {
                    None => continue,
                    Some(Summary::Added) => FileState::Untracked,
                    Some(Summary::Removed) => FileState::Removed,
                    Some(Summary::Modified | Summary::TypeChange) => FileState::Modified,
                    Some(Summary::Renamed | Summary::Copied) => FileState::Renamed,
                    Some(Summary::IntentToAdd) => FileState::Added,
                    Some(Summary::Conflict) => FileState::Conflict,
                };
                (state, false)
            }
        };
        let location = item.location().to_str_lossy();
        let Some(path) = strip(&prefix, &location) else {
            continue;
        };
        files
            .entry(path.to_owned())
            .and_modify(|f| {
                f.staged |= staged;
                if !staged {
                    f.state = state;
                }
            })
            .or_insert(FileStatus {
                path: path.to_owned(),
                state,
                staged,
            });
    }

    Ok(Some(Status {
        branch,
        upstream,
        ahead,
        behind,
        files: files.into_values().collect(),
        log,
        committed,
    }))
}

/// One line of a human-readable `riven.json` diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Added(String),
    Removed(String),
    Changed(String),
}

fn lower(value: impl std::fmt::Debug) -> String {
    format!("{value:?}").to_lowercase()
}

fn file_name(entry: &Entry) -> &str {
    entry.file.path.file_name()
}

/// What changed in the pack between two `riven.json`s: `+ Sodium (sodium-0.6.13.jar)`.
pub fn describe(old: Option<&Project>, new: &Project) -> Vec<DiffLine> {
    let Some(old) = old else {
        return new
            .content
            .iter()
            .map(|e| DiffLine::Added(format!("{} ({})", e.name, file_name(e))))
            .collect();
    };
    let mut out = Vec::new();
    let mut field = |label: &str, a: &str, b: &str| {
        if a != b {
            out.push(DiffLine::Changed(format!("{label} {a} → {b}")));
        }
    };
    field("name", &old.name, &new.name);
    field("version", &old.version, &new.version);
    field("minecraft", &old.minecraft, &new.minecraft);
    field(
        "loader",
        &format!("{} {}", lower(old.loader.kind), old.loader.version),
        &format!("{} {}", lower(new.loader.kind), new.loader.version),
    );
    for g in &new.groups {
        match old.groups.iter().find(|o| o.id == g.id) {
            None => out.push(DiffLine::Added(format!("group {}", g.id))),
            Some(o) if o != g => out.push(DiffLine::Changed(format!("group {}", g.id))),
            Some(_) => {}
        }
    }
    for g in old
        .groups
        .iter()
        .filter(|g| new.groups.iter().all(|n| n.id != g.id))
    {
        out.push(DiffLine::Removed(format!("group {}", g.id)));
    }
    if old.files != new.files {
        out.push(DiffLine::Changed("preserve / ignore rules".into()));
    }
    for e in &new.content {
        let Some(o) = old.entry(&e.id) else {
            out.push(DiffLine::Added(format!("{} ({})", e.name, file_name(e))));
            continue;
        };
        let mut what = Vec::new();
        if o.file.path != e.file.path || o.file.hashes != e.file.hashes {
            what.push(format!("{} → {}", file_name(o), file_name(e)));
        }
        if o.side != e.side {
            what.push(format!("side {} → {}", lower(o.side), lower(e.side)));
        }
        if o.group != e.group {
            what.push(format!(
                "group {} → {}",
                o.group.as_deref().unwrap_or("—"),
                e.group.as_deref().unwrap_or("—")
            ));
        }
        if o.update != e.update {
            what.push(if e.update == UpdatePolicy::Pinned {
                "pinned".into()
            } else {
                "unpinned".into()
            });
        }
        if !what.is_empty() {
            out.push(DiffLine::Changed(format!(
                "{}: {}",
                e.name,
                what.join(", ")
            )));
        }
    }
    for e in old.content.iter().filter(|e| new.entry(&e.id).is_none()) {
        out.push(DiffLine::Removed(format!("{} ({})", e.name, file_name(e))));
    }
    out
}

/// Whether a `git` executable runs.
pub fn available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn auth_failure(output: &str) -> bool {
    let output = output.to_lowercase();
    [
        "authentication failed",
        "permission denied (publickey",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "host key verification failed",
    ]
    .iter()
    .any(|needle| output.contains(needle))
}

/// Splits on `\n` and the `\r` git uses to redraw progress lines.
async fn pump(mut from: impl AsyncRead + Unpin, line: &(dyn Fn(String) + Sync)) -> String {
    let mut all = String::new();
    let mut pending = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = from.read(&mut buf).await {
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            if b == b'\n' || b == b'\r' {
                if !pending.is_empty() {
                    let text = String::from_utf8_lossy(&pending).into_owned();
                    all.push_str(&text);
                    all.push('\n');
                    line(text);
                    pending.clear();
                }
            } else {
                pending.push(b);
            }
        }
    }
    if !pending.is_empty() {
        let text = String::from_utf8_lossy(&pending).into_owned();
        all.push_str(&text);
        line(text);
    }
    all
}

/// Runs system git in `dir` without prompts, streaming its output line by line.
pub async fn run(
    dir: &Path,
    args: &[&str],
    line: &(dyn Fn(String) + Sync),
) -> Result<(), GitError> {
    let mut child = tokio::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => GitError::NoGit,
            _ => GitError::Io {
                path: dir.to_owned(),
                source: e,
            },
        })?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let (out, err) = tokio::join!(pump(stdout, line), pump(stderr, line));
    let status = child.wait().await.map_err(|source| GitError::Io {
        path: dir.to_owned(),
        source,
    })?;
    if status.success() {
        return Ok(());
    }
    let output = format!("{out}{err}");
    if auth_failure(&output) {
        return Err(GitError::Auth);
    }
    let message = output
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no output")
        .trim()
        .to_owned();
    Err(GitError::Failed {
        command: args.first().copied().unwrap_or_default().to_owned(),
        message,
    })
}

/// Stages everything under the project folder and commits it.
pub async fn commit(
    dir: &Path,
    message: &str,
    line: &(dyn Fn(String) + Sync),
) -> Result<(), GitError> {
    run(dir, &["add", "--all", "--", "."], line).await?;
    run(dir, &["commit", "-m", message], line).await
}

pub async fn pull(dir: &Path, line: &(dyn Fn(String) + Sync)) -> Result<(), GitError> {
    run(dir, &["pull", "--ff-only"], line).await
}

/// Pushes the current branch, setting `origin` as its upstream when it has none.
pub async fn push(
    dir: &Path,
    has_upstream: bool,
    line: &(dyn Fn(String) + Sync),
) -> Result<(), GitError> {
    if has_upstream {
        run(dir, &["push"], line).await
    } else {
        run(dir, &["push", "--set-upstream", "origin", "HEAD"], line).await
    }
}

/// Clones `url` into `dest`, which must not exist yet.
pub async fn clone(url: &str, dest: &Path, line: &(dyn Fn(String) + Sync)) -> Result<(), GitError> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| GitError::Io {
        path: parent.to_owned(),
        source,
    })?;
    let dest = dest.to_string_lossy();
    run(parent, &["clone", "--progress", url, &dest], line).await
}

/// The folder name `git clone` picks for `url`.
pub fn clone_name(url: &str) -> Option<String> {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()?
        .trim_end_matches(".git");
    (!last.is_empty()).then(|| last.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_scoped_to_the_project_folder() {
        if !available() {
            return;
        }
        let root = std::env::temp_dir().join(format!("riven-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = root.join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(pack.join("a.txt"), "1").unwrap();
        std::fs::write(root.join("other.txt"), "1").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(pack.join("a.txt"), "2").unwrap();
        std::fs::write(pack.join("b.txt"), "new").unwrap();
        std::fs::write(root.join("other.txt"), "2").unwrap();

        let status = status(&pack).unwrap().unwrap();
        let files: Vec<(&str, FileState)> = status
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.state))
            .collect();
        assert_eq!(
            files,
            [
                ("a.txt", FileState::Modified),
                ("b.txt", FileState::Untracked)
            ]
        );
        assert_eq!(status.log.len(), 1);
        assert!(status.committed.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clone_names_match_git() {
        assert_eq!(
            clone_name("https://github.com/BX-Team/VideCraft.git").as_deref(),
            Some("VideCraft")
        );
        assert_eq!(
            clone_name("git@github.com:BX-Team/pack").as_deref(),
            Some("pack")
        );
    }
}
