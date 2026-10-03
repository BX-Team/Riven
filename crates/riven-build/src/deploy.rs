use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use riven_format::PackPath;

#[derive(Debug, thiserror::Error)]
pub enum DeployError {
    #[error("git is not installed or not in PATH")]
    NoGit,
    #[error("`git {command}` failed: {stderr}")]
    Git { command: String, stderr: String },
    #[error("{0} is not inside a git repository")]
    NotRepo(PathBuf),
    #[error("{0} has nothing to deploy; run `riven build` first")]
    EmptyDist(PathBuf),
    #[error("`{0}` is checked out here; deploy from another branch")]
    CheckedOut(String),
    #[error("local `{branch}` and `{remote}/{branch}` have diverged; reconcile them by hand")]
    Diverged { branch: String, remote: String },
    #[error(
        "{0} is already published with other content; releases are immutable, bump the version"
    )]
    Immutable(String),
    #[error("no git remote `{0}`; add one or deploy with --no-push")]
    NoRemote(String),
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Where and how `dist/` goes to a branch.
pub struct Deploy<'a> {
    pub dist: &'a Path,
    pub branch: &'a str,
    pub remote: &'a str,
    pub message: &'a str,
    pub push: bool,
}

#[derive(Debug)]
pub struct Deployed {
    /// The new commit, `None` when the branch already had exactly this content.
    pub commit: Option<String>,
    /// The branch did not exist anywhere before.
    pub created: bool,
    pub pushed: bool,
    /// Files fetched back from the branch into `dist/`.
    pub restored: usize,
    /// The GitHub Pages address of the remote, if it is a GitHub repository.
    pub pages: Option<String>,
    /// The same as a `gh:owner/repo` install link.
    pub short_link: Option<String>,
}

struct Git<'a> {
    dir: &'a Path,
    index: Option<&'a Path>,
}

impl Git<'_> {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(self.dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(index) = self.index {
            command.env("GIT_INDEX_FILE", index);
        }
        command
    }

    fn bytes(&self, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, DeployError> {
        let mut command = self.command(args);
        if input.is_some() {
            command.stdin(Stdio::piped());
        }
        let mut child = command.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => DeployError::NoGit,
            _ => DeployError::Git {
                command: args.join(" "),
                stderr: e.to_string(),
            },
        })?;
        if let Some(input) = input {
            let mut stdin = child.stdin.take().expect("stdin is piped");
            let input = input.to_vec();
            std::thread::spawn(move || stdin.write_all(&input));
        }
        let output = child.wait_with_output().map_err(|e| DeployError::Git {
            command: args.join(" "),
            stderr: e.to_string(),
        })?;
        if !output.status.success() {
            return Err(DeployError::Git {
                command: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(output.stdout)
    }

    fn run_with(&self, args: &[&str], input: Option<&[u8]>) -> Result<String, DeployError> {
        let stdout = self.bytes(args, input)?;
        Ok(String::from_utf8_lossy(&stdout).trim().to_owned())
    }

    fn run(&self, args: &[&str]) -> Result<String, DeployError> {
        self.run_with(args, None)
    }

    fn succeeds(&self, args: &[&str]) -> Result<bool, DeployError> {
        match self.run(args) {
            Ok(_) => Ok(true),
            Err(DeployError::Git { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn rev(&self, name: &str) -> Result<Option<String>, DeployError> {
        match self.run(&["rev-parse", "--verify", "-q", &format!("{name}^{{commit}}")]) {
            Ok(sha) => Ok(Some(sha)),
            Err(DeployError::Git { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn is_ancestor(&self, a: &str, b: &str) -> Result<bool, DeployError> {
        self.succeeds(&["merge-base", "--is-ancestor", a, b])
    }
}

/// Every file under `dist/` with its git blob id, written to the object store.
fn files(git: &Git, dist: &Path) -> Result<Vec<(String, String)>, DeployError> {
    let mut out = Vec::new();
    let mut stack = vec![(String::new(), dist.to_path_buf())];
    while let Some((prefix, dir)) = stack.pop() {
        let io = |source| DeployError::Io {
            path: dir.clone(),
            source,
        };
        for item in std::fs::read_dir(&dir).map_err(io)? {
            let item = item.map_err(io)?;
            let name = item.file_name().to_string_lossy().into_owned();
            let path = format!("{prefix}{name}");
            let kind = item.file_type().map_err(io)?;
            if kind.is_dir() {
                stack.push((format!("{path}/"), item.path()));
            } else if kind.is_file() {
                out.push((path, item.path()));
            }
        }
    }
    out.sort();
    if out.is_empty() {
        return Ok(vec![]);
    }
    let paths: String = out
        .iter()
        .map(|(_, path)| format!("{}\n", path.display()))
        .collect();
    let shas = git.run_with(
        &["hash-object", "-w", "--stdin-paths"],
        Some(paths.as_bytes()),
    )?;
    Ok(out
        .into_iter()
        .zip(shas.lines())
        .map(|((name, _), sha)| (name, sha.to_owned()))
        .collect())
}

/// Refuses to rewrite published releases and blobs, and copies branch-only files back into `dist/`.
fn restore(
    git: &Git,
    parent: Option<&str>,
    dist: &Path,
    files: &mut Vec<(String, String)>,
) -> Result<usize, DeployError> {
    let published: BTreeMap<String, String> = match parent {
        Some(parent) => git
            .run(&["ls-tree", "-r", "-z", "--full-tree", parent])?
            .split('\0')
            .filter_map(|line| {
                let (meta, path) = line.split_once('\t')?;
                let sha = meta.split(' ').nth(2)?;
                Some((path.to_owned(), sha.to_owned()))
            })
            .collect(),
        None => BTreeMap::new(),
    };
    for (name, sha) in files.iter() {
        let immutable = name.starts_with("releases/") || name.starts_with("blobs/");
        if immutable && published.get(name).is_some_and(|old| old != sha) {
            return Err(DeployError::Immutable(name.clone()));
        }
    }
    let missing: Vec<(&String, &String)> = published
        .iter()
        .filter(|(name, _)| name.as_str() != ".nojekyll")
        .filter(|(name, _)| !files.iter().any(|(have, _)| have == *name))
        .collect();
    if missing.is_empty() {
        return Ok(0);
    }
    let request: String = missing.iter().map(|(_, sha)| format!("{sha}\n")).collect();
    let batch = git.bytes(&["cat-file", "--batch"], Some(request.as_bytes()))?;
    let mut rest = batch.as_slice();
    for (name, sha) in &missing {
        let bad = || DeployError::Git {
            command: "cat-file --batch".into(),
            stderr: format!("unexpected output for {name}"),
        };
        let end = rest.iter().position(|b| *b == b'\n').ok_or_else(bad)?;
        let header = String::from_utf8_lossy(&rest[..end]).into_owned();
        let size: usize = header
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(bad)?;
        let body = rest.get(end + 1..end + 1 + size).ok_or_else(bad)?;
        rest = rest.get(end + 2 + size..).unwrap_or_default();
        let path = PackPath::new(name.as_str()).map_err(|_| bad())?;
        let target = dist.join(path.as_str());
        let io = |source| DeployError::Io {
            path: target.clone(),
            source,
        };
        std::fs::create_dir_all(target.parent().expect("pack paths have a parent")).map_err(io)?;
        std::fs::write(&target, body).map_err(io)?;
        files.push(((*name).clone(), (*sha).clone()));
    }
    files.sort();
    Ok(missing.len())
}

/// `owner` and `repo` of a GitHub remote URL.
pub fn github_repo(remote: &str) -> Option<(&str, &str)> {
    let path = [
        "https://github.com/",
        "ssh://git@github.com/",
        "git@github.com:",
    ]
    .iter()
    .find_map(|prefix| remote.strip_prefix(prefix))?;
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    path.split_once('/').filter(|(_, repo)| !repo.contains('/'))
}

/// `https://owner.github.io/repo/` for a GitHub remote URL.
pub fn pages_url(remote: &str) -> Option<String> {
    let (owner, repo) = github_repo(remote)?;
    let host = format!("{}.github.io", owner.to_ascii_lowercase());
    if repo.eq_ignore_ascii_case(&host) {
        Some(format!("https://{host}/"))
    } else {
        Some(format!("https://{host}/{repo}/"))
    }
}

/// Commits `dist/` over `branch`, keeping files already there, without touching the working tree.
pub fn deploy(dir: &Path, options: &Deploy) -> Result<Deployed, DeployError> {
    let git = Git { dir, index: None };
    let git_dir = match git.run(&["rev-parse", "--absolute-git-dir"]) {
        Ok(path) => PathBuf::from(path),
        Err(DeployError::Git { .. }) => return Err(DeployError::NotRepo(dir.to_path_buf())),
        Err(e) => return Err(e),
    };
    let branch_ref = format!("refs/heads/{}", options.branch);
    if git.run(&["symbolic-ref", "-q", "HEAD"]).ok().as_deref() == Some(branch_ref.as_str()) {
        return Err(DeployError::CheckedOut(options.branch.to_owned()));
    }
    std::fs::create_dir_all(options.dist).map_err(|source| DeployError::Io {
        path: options.dist.to_path_buf(),
        source,
    })?;
    let mut files = files(&git, options.dist)?;

    let remote_url = git.run(&["remote", "get-url", options.remote]).ok();
    let tracking = format!("refs/remotes/{}/{}", options.remote, options.branch);
    let remote = if options.push {
        if remote_url.is_none() {
            return Err(DeployError::NoRemote(options.remote.to_owned()));
        }
        let heads = git.run(&["ls-remote", "--heads", options.remote, &branch_ref])?;
        if heads.is_empty() {
            None
        } else {
            git.run(&[
                "fetch",
                "-q",
                options.remote,
                &format!("+{branch_ref}:{tracking}"),
            ])?;
            git.rev(&tracking)?
        }
    } else {
        git.rev(&tracking)?
    };
    let local = git.rev(&branch_ref)?;
    let parent = match (&local, &remote) {
        (Some(l), Some(r)) if l == r || git.is_ancestor(r, l)? => Some(l.clone()),
        (Some(l), Some(r)) if git.is_ancestor(l, r)? => Some(r.clone()),
        (Some(_), Some(_)) => {
            return Err(DeployError::Diverged {
                branch: options.branch.to_owned(),
                remote: options.remote.to_owned(),
            });
        }
        (l, r) => l.clone().or_else(|| r.clone()),
    };

    if files.is_empty() && parent.is_none() {
        return Err(DeployError::EmptyDist(options.dist.to_path_buf()));
    }
    let restored = restore(&git, parent.as_deref(), options.dist, &mut files)?;

    let index = git_dir.join("riven-deploy-index");
    let _ = std::fs::remove_file(&index);
    let staged = Git {
        dir,
        index: Some(&index),
    };
    let tree = (|| {
        match &parent {
            Some(parent) => staged.run(&["read-tree", parent])?,
            None => staged.run(&["read-tree", "--empty"])?,
        };
        let mut info: String = files
            .iter()
            .map(|(name, sha)| format!("100644 {sha}\t{name}\n"))
            .collect();
        if !files.iter().any(|(name, _)| name == ".nojekyll") {
            let empty = staged.run_with(&["hash-object", "-w", "--stdin"], Some(b""))?;
            info.push_str(&format!("100644 {empty}\t.nojekyll\n"));
        }
        staged.run_with(&["update-index", "--index-info"], Some(info.as_bytes()))?;
        staged.run(&["write-tree"])
    })();
    let _ = std::fs::remove_file(&index);
    let tree = tree?;

    let unchanged = match &parent {
        Some(parent) => git.run(&["rev-parse", &format!("{parent}^{{tree}}")])? == tree,
        None => false,
    };
    let commit = if unchanged {
        None
    } else {
        let mut args = vec!["commit-tree", tree.as_str(), "-m", options.message];
        if let Some(parent) = &parent {
            args.extend(["-p", parent.as_str()]);
        }
        let commit = git.run(&args)?;
        let mut update = vec!["update-ref", branch_ref.as_str(), commit.as_str()];
        if let Some(local) = &local {
            update.push(local.as_str());
        }
        git.run(&update)?;
        Some(commit)
    };

    let head = commit.clone().or(parent.clone());
    let pushed = options.push && head.is_some() && head != remote;
    if pushed {
        git.run(&[
            "push",
            "-q",
            options.remote,
            &format!("{branch_ref}:{branch_ref}"),
        ])?;
    }
    Ok(Deployed {
        commit,
        created: local.is_none() && remote.is_none(),
        pushed,
        restored,
        pages: remote_url.as_deref().and_then(pages_url),
        short_link: remote_url
            .as_deref()
            .and_then(github_repo)
            .map(|(owner, repo)| format!("gh:{owner}/{repo}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        Git { dir, index: None }.run(args).unwrap()
    }

    #[test]
    fn deploys_merge_into_the_branch_and_keep_old_releases() {
        let root = std::env::temp_dir().join(format!("riven-deploy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (origin, work, dist) = (root.join("origin"), root.join("work"), root.join("dist"));
        for dir in [&origin, &work] {
            std::fs::create_dir_all(dir).unwrap();
        }
        git(&origin, &["init", "-q", "--bare"]);
        git(&work, &["init", "-q"]);
        git(&work, &["config", "user.name", "Riven Test"]);
        git(&work, &["config", "user.email", "test@riven.invalid"]);
        git(&work, &["config", "commit.gpgsign", "false"]);
        git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );

        let write = |path: &str, text: &str| {
            let path = dist.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let options = Deploy {
            dist: &dist,
            branch: "gh-pages",
            remote: "origin",
            message: "deploy",
            push: true,
        };

        write("releases/1.0.0.json", "one");
        write("channels/stable.json", "1.0.0");
        let first = deploy(&work, &options).unwrap();
        assert!(first.created && first.pushed && first.commit.is_some());

        std::fs::remove_dir_all(&dist).unwrap();
        write("releases/1.1.0.json", "two");
        write("channels/stable.json", "1.1.0");
        let second = deploy(&work, &options).unwrap();
        assert!(!second.created && second.pushed);
        assert_eq!(second.restored, 1);
        assert_eq!(
            std::fs::read_to_string(dist.join("releases/1.0.0.json")).unwrap(),
            "one"
        );

        let tree = git(&origin, &["ls-tree", "-r", "--name-only", "gh-pages"]);
        assert_eq!(
            tree.lines().collect::<Vec<_>>(),
            [
                ".nojekyll",
                "channels/stable.json",
                "releases/1.0.0.json",
                "releases/1.1.0.json"
            ]
        );
        assert_eq!(
            git(&origin, &["show", "gh-pages:channels/stable.json"]),
            "1.1.0"
        );

        let third = deploy(&work, &options).unwrap();
        assert!(third.commit.is_none() && !third.pushed);

        write("releases/1.0.0.json", "rebuilt");
        assert!(matches!(
            deploy(&work, &options),
            Err(DeployError::Immutable(path)) if path == "releases/1.0.0.json"
        ));
        assert_eq!(git(&work, &["status", "--porcelain"]), "");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn github_remotes_map_to_pages() {
        let cases = [
            (
                "https://github.com/BX-Team/VideCraft.git",
                "https://bx-team.github.io/VideCraft/",
            ),
            (
                "git@github.com:BX-Team/VideCraft.git",
                "https://bx-team.github.io/VideCraft/",
            ),
            (
                "ssh://git@github.com/me/me.github.io",
                "https://me.github.io/",
            ),
        ];
        for (remote, pages) in cases {
            assert_eq!(pages_url(remote).as_deref(), Some(pages));
        }
        assert_eq!(pages_url("https://gitlab.com/a/b.git"), None);
    }
}
