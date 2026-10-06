use std::path::{Path, PathBuf};
use std::time::Duration;

use riven_format::{Entry, FileRules, Hashes, Java, Loader, LoaderKind, PackPath, Project, Source};
use riven_resolve::{Downloaded, Downloader, JarFetcher};
use riven_sources::{Cache, GameMeta, GitHub, Modrinth};
use riven_sync::Store;

use crate::author::AuthorError;

pub const PROJECT_FILE: &str = "riven.json";
/// Where files shipped as they are live: `overrides/{common,client,server}/…`.
pub const OVERRIDES: &str = "overrides";
pub const OVERRIDE_SIDES: [&str; 3] = ["common", "client", "server"];
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// A loaded `riven.json` and the directory it lives in.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub dir: PathBuf,
    pub project: Project,
}

impl Workspace {
    /// Finds `riven.json` in the current directory or its parents.
    pub fn find() -> Result<Self, AuthorError> {
        let cwd = std::env::current_dir().map_err(io(Path::new(".")))?;
        let dir = cwd
            .ancestors()
            .find(|d| d.join(PROJECT_FILE).is_file())
            .ok_or(AuthorError::NoProject(cwd.clone()))?;
        Self::open(dir)
    }

    /// Reads `dir/riven.json`.
    pub fn open(dir: &Path) -> Result<Self, AuthorError> {
        let path = dir.join(PROJECT_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AuthorError::NoProject(dir.to_owned()));
            }
            Err(e) => return Err(io(&path)(e)),
        };
        let project =
            riven_format::from_str(&text).map_err(|source| AuthorError::Format { path, source })?;
        Ok(Self {
            dir: dir.to_owned(),
            project,
        })
    }

    pub fn save(&self) -> Result<(), AuthorError> {
        if let Some(issue) = self.project.validate().first() {
            return Err(AuthorError::Invalid(issue.to_string()));
        }
        write_atomic(
            &self.dir.join(PROJECT_FILE),
            &riven_format::to_string(&self.project),
        )
    }

    pub fn cache(&self) -> Cache {
        Cache::new(self.dir.join(".riven").join("cache"), CACHE_TTL)
    }

    pub fn modrinth(&self) -> Modrinth {
        Modrinth::new(riven_sources::client()).with_cache(self.cache())
    }

    pub fn github(&self) -> GitHub {
        GitHub::new(riven_sources::client()).with_cache(self.cache())
    }

    /// Jar access for this pack: the content store, plus `local` files from the repository.
    pub fn jars(&self) -> Result<StoreJars, AuthorError> {
        StoreJars::new(&self.dir)
    }
}

/// A file or folder under `overrides/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// Relative to the project, e.g. `overrides/common/config/create-common.toml`.
    pub path: PackPath,
    pub dir: bool,
}

impl Workspace {
    /// The `overrides/` tree, folders before files, each level sorted by name; symlinks are left out.
    pub fn overrides(&self) -> Result<Vec<TreeEntry>, AuthorError> {
        fn walk(root: &Path, rel: &str, out: &mut Vec<TreeEntry>) -> Result<(), AuthorError> {
            let dir = root.join(rel);
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(io(&dir)(e)),
            };
            let mut found: Vec<(bool, String)> = Vec::new();
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                found.push((!kind.is_dir(), name));
            }
            found.sort_by(|a, b| {
                a.0.cmp(&b.0)
                    .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
            });
            for (is_file, name) in found {
                let child = format!("{rel}/{name}");
                let Ok(path) = PackPath::new(&child) else {
                    continue;
                };
                out.push(TreeEntry {
                    path,
                    dir: !is_file,
                });
                if !is_file {
                    walk(root, &child, out)?;
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        walk(&self.dir, OVERRIDES, &mut out)?;
        Ok(out)
    }

    /// The on-disk path of a project file, refusing anything outside `overrides/` and symlinks on the way.
    pub fn override_path(&self, path: &PackPath) -> Result<PathBuf, AuthorError> {
        let inside = path
            .as_str()
            .strip_prefix(OVERRIDES)
            .is_some_and(|rest| rest.starts_with('/'));
        if !inside {
            return Err(AuthorError::NotOverride(path.clone()));
        }
        riven_sync::install::safe_target(&self.dir, path)
            .map_err(|_| AuthorError::NotOverride(path.clone()))
    }

    pub fn read_override(&self, path: &PackPath) -> Result<String, AuthorError> {
        let full = self.override_path(path)?;
        let bytes = std::fs::read(&full).map_err(io(&full))?;
        String::from_utf8(bytes).map_err(|_| AuthorError::Binary(path.clone()))
    }

    /// Writes a file through a temporary one, creating its folders.
    pub fn write_override(&self, path: &PackPath, text: &str) -> Result<(), AuthorError> {
        let full = self.override_path(path)?;
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        let tmp = full.with_file_name(format!(".{}.tmp", path.file_name()));
        std::fs::write(&tmp, text).map_err(io(&tmp))?;
        std::fs::rename(&tmp, &full).map_err(io(&full))
    }

    /// Creates an empty file, refusing to replace one.
    pub fn create_override(&self, path: &PackPath) -> Result<(), AuthorError> {
        let full = self.override_path(path)?;
        if full.exists() {
            return Err(AuthorError::Exists(full));
        }
        self.write_override(path, "")
    }

    pub fn create_override_dir(&self, path: &PackPath) -> Result<(), AuthorError> {
        let full = self.override_path(path)?;
        std::fs::create_dir_all(&full).map_err(io(&full))
    }

    /// Deletes a file, or a folder with everything in it.
    pub fn delete_override(&self, path: &PackPath) -> Result<(), AuthorError> {
        let full = self.override_path(path)?;
        let meta = std::fs::symlink_metadata(&full).map_err(io(&full))?;
        if meta.is_dir() {
            std::fs::remove_dir_all(&full).map_err(io(&full))
        } else {
            std::fs::remove_file(&full).map_err(io(&full))
        }
    }
}

pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> AuthorError + '_ {
    move |source| AuthorError::Io {
        path: path.to_owned(),
        source,
    }
}

pub fn write_atomic(path: &Path, contents: &str) -> Result<(), AuthorError> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
}

pub fn game_meta(cache: Option<Cache>) -> GameMeta {
    let game = GameMeta::new(riven_sources::client());
    match cache {
        Some(cache) => game.with_cache(cache),
        None => game,
    }
}

/// Adds riven's working folders to the repository's `.gitignore`.
pub fn ensure_gitignore(dir: &Path) -> Result<(), AuthorError> {
    let path = dir.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = [".riven/", "dist/", "exports/"]
        .into_iter()
        .filter(|line| !existing.lines().any(|l| l.trim() == *line))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut contents = existing;
    if !contents.is_empty() && !contents.ends_with('\n') {
        contents.push('\n');
    }
    for line in missing {
        contents.push_str(line);
        contents.push('\n');
    }
    std::fs::write(&path, contents).map_err(io(&path))
}

fn slugify(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "modpack".into()
    } else {
        slug
    }
}

/// Creates `dir/riven.json` for the newest (or a given) Minecraft and loader build.
pub async fn init(
    dir: &Path,
    minecraft: Option<String>,
    loader: LoaderKind,
    loader_version: Option<String>,
) -> Result<Workspace, AuthorError> {
    let path = dir.join(PROJECT_FILE);
    if path.exists() {
        return Err(AuthorError::Exists(path));
    }
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let game = game_meta(Some(Cache::new(
        dir.join(".riven").join("cache"),
        CACHE_TTL,
    )));
    let minecraft = match minecraft {
        Some(mc) => mc,
        None => game.latest_minecraft().await?,
    };
    let java = game.java_major(&minecraft).await?;
    let loader_version = match loader_version {
        Some(v) => v,
        None => game.loader_version(loader, &minecraft).await?,
    };
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "modpack".into());
    let project = Project {
        id: slugify(&name),
        name,
        version: "0.1.0".into(),
        authors: vec![],
        description: None,
        icon: None,
        minecraft,
        loader: Loader {
            kind: loader,
            version: loader_version,
        },
        java: Java {
            major: java,
            memory: None,
            jvm_args: vec![],
        },
        groups: vec![],
        files: FileRules::default(),
        content: vec![],
    };
    write_atomic(&path, &riven_format::to_string(&project))?;
    ensure_gitignore(dir)?;
    Ok(Workspace {
        dir: dir.to_owned(),
        project,
    })
}

/// Fetches jars through the shared content store; `local` entries come from the pack repository.
pub struct StoreJars {
    store: Store,
    http: reqwest::Client,
    repo: PathBuf,
}

impl StoreJars {
    pub fn new(repo: &Path) -> Result<Self, AuthorError> {
        Ok(Self {
            store: Store::default_location().ok_or(AuthorError::NoDataDir)?,
            http: riven_sources::client(),
            repo: repo.to_owned(),
        })
    }

    /// Adds bytes to the store, returning them with their computed hashes.
    pub fn keep(&self, bytes: Vec<u8>, origin: &str) -> Result<Downloaded, String> {
        let stored = self
            .store
            .insert(&bytes, &Hashes::default(), origin)
            .map_err(|e| e.to_string())?;
        Ok(Downloaded {
            bytes,
            hashes: Hashes {
                sha512: Some(stored.sha512),
                sha1: Some(stored.sha1),
            },
            size: stored.size,
        })
    }

    /// Where `entry`'s bytes are on disk, if already present.
    pub fn local_path(&self, entry: &Entry) -> Option<PathBuf> {
        match &entry.source {
            Source::Local { path } => Some(self.repo.join(path.as_str())),
            _ => self.store.get(&entry.file.hashes),
        }
    }
}

impl Downloader for StoreJars {
    async fn download(&self, url: &str) -> Result<Downloaded, String> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| e.to_string())?;
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;
        self.keep(bytes.to_vec(), url)
    }
}

impl JarFetcher for StoreJars {
    async fn jar(&self, entry: &Entry) -> Result<Vec<u8>, String> {
        if let Source::Local { path } = &entry.source {
            let path = self.repo.join(path.as_str());
            return tokio::fs::read(&path)
                .await
                .map_err(|e| format!("{}: {e}", path.display()));
        }
        let file = &entry.file;
        let path = match self.store.get(&file.hashes) {
            Some(path) => path,
            None => {
                let urls: Vec<String> = file.url.iter().cloned().collect();
                self.store
                    .fetch(&self.http, &urls, &file.hashes)
                    .await
                    .map_err(|e| e.to_string())?
                    .path
            }
        };
        tokio::fs::read(&path).await.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!("riven-ws-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("overrides/common/config")).unwrap();
        Workspace {
            dir,
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
                files: FileRules::default(),
                content: vec![],
            },
        }
    }

    #[test]
    fn override_files_stay_inside_overrides() {
        let ws = workspace("inside");
        let path = |p: &str| PackPath::new(p).unwrap();
        ws.write_override(&path("overrides/common/config/a.toml"), "a = 1")
            .unwrap();
        assert_eq!(
            ws.read_override(&path("overrides/common/config/a.toml"))
                .unwrap(),
            "a = 1"
        );
        for outside in [
            "riven.json",
            "overrides",
            "overridesx/a.toml",
            "local/a.jar",
        ] {
            assert!(
                matches!(
                    ws.override_path(&path(outside)),
                    Err(AuthorError::NotOverride(_))
                ),
                "{outside}"
            );
        }
        let tree: Vec<(String, bool)> = ws
            .overrides()
            .unwrap()
            .into_iter()
            .map(|e| (e.path.as_str().to_owned(), e.dir))
            .collect();
        assert_eq!(
            tree,
            [
                ("overrides/common".to_owned(), true),
                ("overrides/common/config".to_owned(), true),
                ("overrides/common/config/a.toml".to_owned(), false),
            ]
        );
        let _ = std::fs::remove_dir_all(&ws.dir);
    }

    #[cfg(unix)]
    #[test]
    fn override_writes_refuse_symlinks() {
        let ws = workspace("symlink");
        let outside = ws.dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, ws.dir.join("overrides/client")).unwrap();
        let path = PackPath::new("overrides/client/evil.txt").unwrap();
        assert!(ws.write_override(&path, "x").is_err());
        assert!(!outside.join("evil.txt").exists());
        assert!(
            ws.overrides()
                .unwrap()
                .iter()
                .all(|e| !e.path.as_str().contains("client"))
        );
        let _ = std::fs::remove_dir_all(&ws.dir);
    }
}
