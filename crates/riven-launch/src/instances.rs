use std::path::{Path, PathBuf};

use riven_format::{Instance, LaunchOverrides, Loader};

use crate::{LaunchError, io};

const INSTANCE_FILE: &str = "instance.json";
const GAME_DIR: &str = "minecraft";

/// Instances under `<data>/riven/instances/<id>/`: `instance.json` plus the game directory.
#[derive(Debug, Clone)]
pub struct Instances {
    root: PathBuf,
}

impl Instances {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_location() -> Result<Self, LaunchError> {
        let data = riven_sync::data_dir().ok_or(LaunchError::NoDir("data"))?;
        Ok(Self::new(data.join("instances")))
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Where the game runs: mods, saves, configs and the pack's `.riven/state.json`.
    pub fn game_dir(&self, id: &str) -> PathBuf {
        self.dir(id).join(GAME_DIR)
    }

    /// Every instance by id, sorted by name; unreadable ones are skipped.
    pub fn list(&self) -> Result<Vec<(String, Instance)>, LaunchError> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(io(&self.root)(e)),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            match self.load(&id) {
                Ok(instance) => out.push((id, instance)),
                Err(LaunchError::NoInstance(_)) => {}
                Err(e) => tracing::warn!("skipping instance `{id}`: {e}"),
            }
        }
        out.sort_by_key(|(_, i)| i.name.to_lowercase());
        Ok(out)
    }

    pub fn load(&self, id: &str) -> Result<Instance, LaunchError> {
        let path = self.dir(id).join(INSTANCE_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                riven_format::from_str(&text).map_err(|source| LaunchError::Format { path, source })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(LaunchError::NoInstance(id.to_owned()))
            }
            Err(e) => Err(io(&path)(e)),
        }
    }

    pub fn save(&self, id: &str, instance: &Instance) -> Result<(), LaunchError> {
        crate::save(&self.dir(id).join(INSTANCE_FILE), instance)
    }

    /// Creates an empty instance and returns its id, derived from `name`.
    pub fn create(
        &self,
        name: &str,
        minecraft: &str,
        loader: Option<Loader>,
    ) -> Result<String, LaunchError> {
        let id = unique_id(&self.root, name);
        let instance = Instance {
            name: name.to_owned(),
            minecraft: minecraft.to_owned(),
            loader,
            overrides: LaunchOverrides::default(),
            own_mods: false,
            modrinth: None,
            last_played: None,
            play_seconds: 0,
        };
        let game = self.game_dir(&id);
        std::fs::create_dir_all(&game).map_err(io(&game))?;
        self.save(&id, &instance)?;
        Ok(id)
    }

    /// Creates an empty instance with a fixed id unless it exists; `true` when it was created.
    pub fn ensure(
        &self,
        id: &str,
        name: &str,
        minecraft: &str,
        loader: Option<Loader>,
    ) -> Result<bool, LaunchError> {
        if self.dir(id).join(INSTANCE_FILE).is_file() {
            return Ok(false);
        }
        let instance = Instance {
            name: name.to_owned(),
            minecraft: minecraft.to_owned(),
            loader,
            overrides: LaunchOverrides::default(),
            own_mods: false,
            modrinth: None,
            last_played: None,
            play_seconds: 0,
        };
        let game = self.game_dir(id);
        std::fs::create_dir_all(&game).map_err(io(&game))?;
        self.save(id, &instance)?;
        Ok(true)
    }

    /// Copies an instance under a new name, keeping links to shared folders as links.
    pub fn duplicate(&self, id: &str, name: &str) -> Result<String, LaunchError> {
        let mut instance = self.load(id)?;
        let new_id = unique_id(&self.root, name);
        copy_tree(&self.dir(id), &self.dir(&new_id))?;
        instance.name = name.to_owned();
        instance.last_played = None;
        instance.play_seconds = 0;
        self.save(&new_id, &instance)?;
        Ok(new_id)
    }

    /// The instance's icon, `icon-<hash>.png` in its folder, when it has one.
    pub fn icon(&self, id: &str) -> Option<PathBuf> {
        std::fs::read_dir(self.dir(id))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("icon-") && n.ends_with(".png"))
            })
    }

    /// Replaces the icon; the file is named by its hash so path-keyed image caches see the change.
    pub fn set_icon(&self, id: &str, png: &[u8]) -> Result<PathBuf, LaunchError> {
        self.write_icon(id, png, "icon")
    }

    /// The icon a pack ships; the player cannot replace it while the pack provides one.
    pub fn set_pack_icon(&self, id: &str, png: &[u8]) -> Result<PathBuf, LaunchError> {
        self.write_icon(id, png, "icon-pack")
    }

    pub fn has_pack_icon(&self, id: &str) -> bool {
        self.icon(id).is_some_and(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("icon-pack-"))
        })
    }

    fn write_icon(&self, id: &str, png: &[u8], prefix: &str) -> Result<PathBuf, LaunchError> {
        self.clear_icon(id)?;
        let path = self
            .dir(id)
            .join(format!("{prefix}-{}.png", crate::short_hash(png)));
        std::fs::write(&path, png).map_err(io(&path))?;
        Ok(path)
    }

    pub fn clear_icon(&self, id: &str) -> Result<(), LaunchError> {
        while let Some(old) = self.icon(id) {
            std::fs::remove_file(&old).map_err(io(&old))?;
        }
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<(), LaunchError> {
        let dir = self.dir(id);
        std::fs::remove_dir_all(&dir).map_err(io(&dir))
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), LaunchError> {
    std::fs::create_dir_all(to).map_err(io(to))?;
    for entry in std::fs::read_dir(from).map_err(io(from))? {
        let entry = entry.map_err(io(from))?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(io(&src))?;
        if kind.is_symlink() {
            let target = std::fs::read_link(&src).map_err(io(&src))?;
            crate::link_dir(&target, &dst).map_err(io(&dst))?;
        } else if kind.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(io(&src))?;
        }
    }
    Ok(())
}

/// A lowercase ASCII slug of `name` not yet taken under `root` (`name`, `name-2`, …).
fn unique_id(root: &Path, name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let base: Vec<&str> = slug.split('-').filter(|s| !s.is_empty()).collect();
    let base = if base.is_empty() {
        "instance".to_owned()
    } else {
        base.join("-")
    };
    (1..)
        .map(|n| match n {
            1 => base.clone(),
            n => format!("{base}-{n}"),
        })
        .find(|id| !root.join(id).exists())
        .expect("some suffix is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_ascii_slugs_and_never_collide() {
        let root = std::env::temp_dir().join(format!("riven-instances-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Instances::new(&root);
        let a = store.create("VideCraft: Create", "1.21.1", None).unwrap();
        let b = store.create("VideCraft: Create", "1.21.1", None).unwrap();
        let c = store.create("Ванилла", "1.21.1", None).unwrap();
        assert_eq!(
            (a.as_str(), b.as_str(), c.as_str()),
            ("videcraft-create", "videcraft-create-2", "instance")
        );
        assert!(store.game_dir(&a).is_dir());
        let names: Vec<String> = store
            .list()
            .unwrap()
            .into_iter()
            .map(|(_, i)| i.name)
            .collect();
        assert_eq!(names, ["VideCraft: Create", "VideCraft: Create", "Ванилла"]);
        let _ = std::fs::remove_dir_all(root);
    }
}
