use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use riven_format::PackPath;
use sha2::{Digest, Sha512};

use crate::author::AuthorError;
use crate::glob;
use crate::import::Scope;
use crate::workspace::{OVERRIDES, Workspace, io};

/// Folders of a game directory whose new files are worth offering back to the pack.
const CONFIG_ROOTS: [&str; 3] = ["config", "defaultconfigs", "kubejs"];
/// Loose files in the game directory root offered the same way.
const CONFIG_FILES: [&str; 1] = ["options.txt"];

/// A file of a test instance that differs from the pack's overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    /// Relative to the game directory, and so to `overrides/<scope>/`.
    pub path: PackPath,
    /// Where the file goes back to: the scope it came from, or a guess for new files.
    pub scope: Scope,
    /// Not in the pack yet; otherwise the pack ships it and the game changed it.
    pub new: bool,
}

fn sha512_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex::encode(Sha512::digest(&bytes)))
}

/// Which scopes of `overrides/` hold each path, from the project's override tree.
fn shipped(ws: &Workspace) -> Result<BTreeMap<String, Vec<Scope>>, AuthorError> {
    let mut out: BTreeMap<String, Vec<Scope>> = BTreeMap::new();
    for entry in ws.overrides()?.into_iter().filter(|e| !e.dir) {
        let rest = &entry.path.as_str()[OVERRIDES.len() + 1..];
        let Some((scope, path)) = rest.split_once('/') else {
            continue;
        };
        let scope = match scope {
            "common" => Scope::Common,
            "client" => Scope::Client,
            "server" => Scope::Server,
            _ => continue,
        };
        out.entry(path.to_owned()).or_default().push(scope);
    }
    Ok(out)
}

fn walk(game: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(game.join(rel)) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let child = format!("{rel}/{}", entry.file_name().to_string_lossy());
        if kind.is_dir() {
            walk(game, &child, out);
        } else if kind.is_file() {
            out.push(child);
        }
    }
}

/// Override files the game changed, and new config files it created, in a client game directory.
pub fn drift(ws: &Workspace, game_dir: &Path) -> Result<Vec<Drift>, AuthorError> {
    let installed = riven_sync::install::load_state(game_dir)
        .map_err(|e| AuthorError::Invalid(e.to_string()))?
        .map(|s| s.files)
        .unwrap_or_default();
    let shipped = shipped(ws)?;
    let mut out = Vec::new();
    for (path, scopes) in &shipped {
        let Ok(pack) = PackPath::new(path) else {
            continue;
        };
        let Some(recorded) = installed.get(&pack) else {
            continue;
        };
        let on_disk = sha512_file(&game_dir.join(path));
        if on_disk.is_some_and(|h| h != recorded.sha512) {
            let scope = [Scope::Client, Scope::Common, Scope::Server]
                .into_iter()
                .find(|s| scopes.contains(s))
                .unwrap_or(Scope::Common);
            out.push(Drift {
                path: pack,
                scope,
                new: false,
            });
        }
    }
    let mut found = Vec::new();
    for root in CONFIG_ROOTS {
        walk(game_dir, root, &mut found);
    }
    found.extend(
        CONFIG_FILES
            .iter()
            .filter(|f| game_dir.join(f).is_file())
            .map(|f| (*f).to_owned()),
    );
    for path in found {
        if shipped.contains_key(&path) || glob::any(&ws.project.files.ignore, &path) {
            continue;
        }
        let Ok(pack) = PackPath::new(&path) else {
            continue;
        };
        if installed.contains_key(&pack) {
            continue;
        }
        let scope = if CONFIG_FILES.contains(&path.as_str()) {
            Scope::Client
        } else {
            Scope::Common
        };
        out.push(Drift {
            path: pack,
            scope,
            new: true,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Copies the picked files from the game directory into `overrides/<scope>/`.
pub fn take(ws: &Workspace, game_dir: &Path, picked: &[Drift]) -> Result<usize, AuthorError> {
    for drift in picked {
        let target = PackPath::new(format!("{OVERRIDES}/{}/{}", drift.scope.dir(), drift.path))?;
        let to: PathBuf = ws.override_path(&target)?;
        let from = riven_sync::install::safe_target(game_dir, &drift.path)
            .map_err(|_| AuthorError::NotOverride(drift.path.clone()))?;
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::copy(&from, &to).map_err(io(&from))?;
    }
    Ok(picked.len())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use riven_format::{
        FileRules, InstallSide, Java, Loader, LoaderKind, Project, State, StateFile,
    };

    use super::*;

    #[test]
    fn changed_and_new_configs_drift_but_untouched_ones_do_not() {
        let root = std::env::temp_dir().join(format!("riven-pull-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (repo, game) = (root.join("repo"), root.join("game"));
        let write = |path: PathBuf, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(repo.join("overrides/common/config/a.toml"), "a = 1");
        write(repo.join("overrides/client/config/b.toml"), "b = 1");
        write(repo.join("overrides/common/config/b.toml"), "b = 0");
        write(game.join("config/a.toml"), "a = 1");
        write(game.join("config/b.toml"), "b = 2");
        write(game.join("config/fresh/c.json"), "{}");
        write(game.join("config/skip.bak"), "");
        write(game.join("mods/x.jar"), "");
        let hash = |t: &str| hex::encode(Sha512::digest(t.as_bytes()));
        let files = BTreeMap::from([
            (
                PackPath::new("config/a.toml").unwrap(),
                StateFile {
                    sha512: hash("a = 1"),
                    preserve: false,
                },
            ),
            (
                PackPath::new("config/b.toml").unwrap(),
                StateFile {
                    sha512: hash("b = 1"),
                    preserve: false,
                },
            ),
        ]);
        riven_sync::install::save_state(
            &game,
            &State {
                source: "pack.riven".into(),
                key: None,
                side: InstallSide::Client,
                version: "1.0.0".into(),
                etag: None,
                groups: BTreeMap::new(),
                files,
            },
        )
        .unwrap();
        let ws = Workspace {
            dir: repo.clone(),
            project: Project {
                name: "Pack".into(),
                id: "pack".into(),
                version: "1.0.0".into(),
                authors: vec![],
                description: None,
                icon: None,
                minecraft: "1.21.1".into(),
                loader: Loader {
                    kind: LoaderKind::Fabric,
                    version: "0.16.0".into(),
                },
                java: Java {
                    major: 21,
                    memory: None,
                    jvm_args: vec![],
                },
                groups: vec![],
                files: FileRules {
                    preserve: vec![],
                    ignore: vec!["**/*.bak".into()],
                },
                content: vec![],
            },
        };

        let found = drift(&ws, &game).unwrap();
        let summary: Vec<(&str, Scope, bool)> = found
            .iter()
            .map(|d| (d.path.as_str(), d.scope, d.new))
            .collect();
        assert_eq!(
            summary,
            [
                ("config/b.toml", Scope::Client, false),
                ("config/fresh/c.json", Scope::Common, true),
            ]
        );
        take(&ws, &game, &found).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("overrides/client/config/b.toml")).unwrap(),
            "b = 2"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("overrides/common/config/b.toml")).unwrap(),
            "b = 0"
        );
        assert!(repo.join("overrides/common/config/fresh/c.json").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
