use std::io::{Cursor, Read as _};
use std::path::{Component, Path};

use riven_format::{Loader, LoaderKind};
use serde::Deserialize;

use crate::instances::Instances;
use crate::{LaunchError, io};

#[derive(Deserialize)]
struct MmcPack {
    components: Vec<PackComponent>,
}

#[derive(Deserialize)]
struct PackComponent {
    uid: String,
    #[serde(default)]
    version: String,
}

/// What a Prism (or MultiMC) export says about its instance.
#[derive(Debug, PartialEq, Eq)]
struct Layout {
    /// The folder in the zip holding `mmc-pack.json`, `""` or `name/`.
    root: String,
    name: Option<String>,
    minecraft: String,
    loader: Option<Loader>,
}

fn bad(zip: &Path, why: impl std::fmt::Display) -> LaunchError {
    LaunchError::BadArchive(format!("{}: {why}", zip.display()))
}

fn read_text(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Option<String> {
    let mut file = archive.by_name(name).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    Some(text)
}

fn layout(
    archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
    zip: &Path,
) -> Result<Layout, LaunchError> {
    let pack = archive
        .file_names()
        .filter_map(Result::ok)
        .filter(|n| n.ends_with("mmc-pack.json"))
        .min_by_key(|n| n.len())
        .map(std::borrow::Cow::into_owned)
        .ok_or_else(|| bad(zip, "no mmc-pack.json; not a Prism or MultiMC export"))?;
    let root = pack.trim_end_matches("mmc-pack.json").to_owned();
    if !(root.is_empty() || root.matches('/').count() == 1) {
        return Err(bad(zip, "mmc-pack.json is nested too deep"));
    }
    let text = read_text(archive, &pack).ok_or_else(|| bad(zip, "cannot read mmc-pack.json"))?;
    let parsed: MmcPack = serde_json::from_str(&text).map_err(|e| bad(zip, e))?;
    let version = |uid: &str| {
        parsed
            .components
            .iter()
            .find(|c| c.uid == uid)
            .map(|c| c.version.clone())
    };
    let minecraft = version("net.minecraft")
        .ok_or_else(|| bad(zip, "no Minecraft version in mmc-pack.json"))?;
    let loader = [
        ("net.neoforged", LoaderKind::NeoForge),
        ("net.minecraftforge", LoaderKind::Forge),
        ("net.fabricmc.fabric-loader", LoaderKind::Fabric),
        ("org.quiltmc.quilt-loader", LoaderKind::Quilt),
    ]
    .into_iter()
    .find_map(|(uid, kind)| {
        Some(Loader {
            kind,
            version: version(uid)?,
        })
    });
    let name = read_text(archive, &format!("{root}instance.cfg")).and_then(|cfg| {
        cfg.lines()
            .find_map(|l| l.strip_prefix("name="))
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
    });
    Ok(Layout {
        root,
        name,
        minecraft,
        loader,
    })
}

/// The game-folder path of a zip entry under `<root>.minecraft/` or `<root>minecraft/`; anything else is skipped.
fn game_path<'a>(entry: &'a str, root: &str) -> Option<&'a Path> {
    let rest = entry.strip_prefix(root)?;
    let inner = rest
        .strip_prefix(".minecraft/")
        .or_else(|| rest.strip_prefix("minecraft/"))?;
    let path = Path::new(inner);
    let normal = path.components().all(|c| matches!(c, Component::Normal(_)));
    (normal && !inner.is_empty() && !inner.contains('\\')).then_some(path)
}

/// Creates an instance from a Prism or MultiMC export zip and returns its id.
pub fn import(store: &Instances, zip: &Path) -> Result<String, LaunchError> {
    let bytes = std::fs::read(zip).map_err(io(zip))?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| bad(zip, e))?;
    let layout = layout(&mut archive, zip)?;
    let fallback = zip
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Prism".into());
    let name = layout.name.clone().unwrap_or(fallback);
    let id = store.create(&name, &layout.minecraft, layout.loader.clone())?;
    let game = store.game_dir(&id);
    let extracted = (|| {
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| bad(zip, e))?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().map_err(|e| bad(zip, e))?;
            let Some(rel) = game_path(&name, &layout.root).map(Path::to_owned) else {
                continue;
            };
            let target = game.join(rel);
            let parent = target.parent().expect("joined paths have a parent");
            std::fs::create_dir_all(parent).map_err(io(parent))?;
            let mut out = std::fs::File::create(&target).map_err(io(&target))?;
            std::io::copy(&mut file, &mut out).map_err(io(&target))?;
        }
        Ok(())
    })();
    if let Err(e) = extracted {
        let _ = store.delete(&id);
        return Err(e);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn zip(files: &[(&str, &str)]) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        zip::ZipArchive::new(Cursor::new(writer.finish().unwrap().into_inner())).unwrap()
    }

    #[test]
    fn reads_nested_exports_and_skips_escapes() {
        let pack = r#"{"components":[{"uid":"net.minecraft","version":"1.21.1"},
            {"uid":"net.fabricmc.intermediary","version":"1.21.1"},
            {"uid":"net.fabricmc.fabric-loader","version":"0.16.5"}]}"#;
        let mut archive = zip(&[
            ("My Pack/mmc-pack.json", pack),
            (
                "My Pack/instance.cfg",
                "InstanceType=OneSix\nname=My Pack\n",
            ),
            ("My Pack/.minecraft/mods/a.jar", "x"),
        ]);
        let found = layout(&mut archive, Path::new("x.zip")).unwrap();
        assert_eq!(found.root, "My Pack/");
        assert_eq!(found.name.as_deref(), Some("My Pack"));
        assert_eq!(found.minecraft, "1.21.1");
        assert_eq!(
            found.loader,
            Some(Loader {
                kind: LoaderKind::Fabric,
                version: "0.16.5".into()
            })
        );
        assert_eq!(
            game_path("My Pack/.minecraft/mods/a.jar", "My Pack/"),
            Some(Path::new("mods/a.jar"))
        );
        assert_eq!(game_path("My Pack/.minecraft/../evil", "My Pack/"), None);
        assert_eq!(game_path("My Pack/minecraft//abs", "My Pack/"), None);
        assert_eq!(game_path("Other/.minecraft/mods/a.jar", "My Pack/"), None);
        assert_eq!(game_path("My Pack/instance.cfg", "My Pack/"), None);
    }
}
