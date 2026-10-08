use std::cmp::Ordering;

use riven_format::LoaderKind;
use serde::Deserialize;

use crate::http::get_json;
use crate::{Cache, Error, Result};

const MOJANG_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions/loader";
const QUILT_META: &str = "https://meta.quiltmc.org/v3/versions/loader";
const NEOFORGE_VERSIONS: &str =
    "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge";
const FORGE_PROMOTIONS: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const FORGE_VERSIONS: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json";

/// Minecraft and mod loader version metadata.
#[derive(Debug, Clone)]
pub struct GameMeta {
    http: reqwest::Client,
    cache: Option<Cache>,
}

#[derive(Deserialize)]
struct Manifest {
    latest: Latest,
    versions: Vec<ManifestVersion>,
}

#[derive(Deserialize)]
struct Latest {
    release: String,
}

#[derive(Deserialize)]
struct ManifestVersion {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    url: String,
}

impl GameMeta {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http, cache: None }
    }

    pub fn with_cache(mut self, cache: Cache) -> Self {
        self.cache = Some(cache);
        self
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        get_json(&self.http, self.cache.as_ref(), url).await
    }

    async fn manifest(&self) -> Result<Manifest> {
        self.get(MOJANG_MANIFEST).await
    }

    pub async fn latest_minecraft(&self) -> Result<String> {
        Ok(self.manifest().await?.latest.release)
    }

    /// Release versions of Minecraft, newest first.
    pub async fn minecraft_releases(&self) -> Result<Vec<String>> {
        Ok(self
            .manifest()
            .await?
            .versions
            .into_iter()
            .filter(|v| v.kind == "release")
            .map(|v| v.id)
            .collect())
    }

    /// Java major version Mojang ships for `minecraft`.
    pub async fn java_major(&self, minecraft: &str) -> Result<u32> {
        #[derive(Deserialize)]
        struct VersionJson {
            #[serde(rename = "javaVersion")]
            java: Option<JavaVersion>,
        }
        #[derive(Deserialize)]
        struct JavaVersion {
            #[serde(rename = "majorVersion")]
            major: u32,
        }
        let manifest = self.manifest().await?;
        let version = manifest
            .versions
            .iter()
            .find(|v| v.id == minecraft)
            .ok_or_else(|| Error::NotFound(format!("Minecraft {minecraft}")))?;
        let json: VersionJson = self.get(&version.url).await?;
        // Versions older than 1.17 predate the field and run on Java 8.
        Ok(json.java.map_or(8, |j| j.major))
    }

    /// The recommended (or newest stable) loader version for `minecraft`.
    pub async fn loader_version(&self, loader: LoaderKind, minecraft: &str) -> Result<String> {
        let none = || {
            Error::NotFound(format!(
                "{} for Minecraft {minecraft}",
                crate::loader_name(loader)
            ))
        };
        match loader {
            LoaderKind::Fabric | LoaderKind::Quilt => {
                #[derive(Deserialize)]
                struct Entry {
                    loader: LoaderEntry,
                }
                #[derive(Deserialize)]
                struct LoaderEntry {
                    version: String,
                    stable: Option<bool>,
                }
                let base = if loader == LoaderKind::Fabric {
                    FABRIC_META
                } else {
                    QUILT_META
                };
                let entries: Vec<Entry> = self.get(&format!("{base}/{minecraft}")).await?;
                let stable = entries
                    .iter()
                    .filter(|e| e.loader.stable.unwrap_or(!e.loader.version.contains('-')))
                    .map(|e| e.loader.version.as_str())
                    .max_by(|a, b| numeric_cmp(a, b));
                stable
                    .or(entries.first().map(|e| e.loader.version.as_str()))
                    .map(str::to_owned)
                    .ok_or_else(none)
            }
            LoaderKind::NeoForge => {
                #[derive(Deserialize)]
                struct Versions {
                    versions: Vec<String>,
                }
                let all: Versions = self.get(NEOFORGE_VERSIONS).await?;
                let prefix = neoforge_prefix(minecraft);
                let matching: Vec<&String> = all
                    .versions
                    .iter()
                    .filter(|v| v.starts_with(&prefix))
                    .collect();
                let release = matching
                    .iter()
                    .filter(|v| !v.contains('-'))
                    .max_by(|a, b| numeric_cmp(a, b));
                release
                    .or_else(|| matching.iter().max_by(|a, b| numeric_cmp(a, b)))
                    .map(|v| v.to_string())
                    .ok_or_else(none)
            }
            LoaderKind::Forge => {
                #[derive(Deserialize)]
                struct Promotions {
                    promos: std::collections::HashMap<String, String>,
                }
                let promotions: Promotions = self.get(FORGE_PROMOTIONS).await?;
                ["recommended", "latest"]
                    .iter()
                    .find_map(|kind| promotions.promos.get(&format!("{minecraft}-{kind}")))
                    .cloned()
                    .ok_or_else(none)
            }
        }
    }
}

impl GameMeta {
    /// Every build of `loader` for `minecraft`, newest first.
    pub async fn loader_versions(
        &self,
        loader: LoaderKind,
        minecraft: &str,
    ) -> Result<Vec<String>> {
        let mut versions: Vec<String> = match loader {
            LoaderKind::Fabric | LoaderKind::Quilt => {
                #[derive(Deserialize)]
                struct Entry {
                    loader: LoaderEntry,
                }
                #[derive(Deserialize)]
                struct LoaderEntry {
                    version: String,
                }
                let base = if loader == LoaderKind::Fabric {
                    FABRIC_META
                } else {
                    QUILT_META
                };
                let entries: Vec<Entry> = self.get(&format!("{base}/{minecraft}")).await?;
                entries.into_iter().map(|e| e.loader.version).collect()
            }
            LoaderKind::NeoForge => {
                #[derive(Deserialize)]
                struct Versions {
                    versions: Vec<String>,
                }
                let all: Versions = self.get(NEOFORGE_VERSIONS).await?;
                let prefix = neoforge_prefix(minecraft);
                all.versions
                    .into_iter()
                    .filter(|v| v.starts_with(&prefix))
                    .collect()
            }
            LoaderKind::Forge => {
                let mut all: std::collections::HashMap<String, Vec<String>> =
                    self.get(FORGE_VERSIONS).await?;
                let prefix = format!("{minecraft}-");
                all.remove(minecraft)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|v| v.strip_prefix(&prefix).map(str::to_owned).unwrap_or(v))
                    .collect()
            }
        };
        versions.sort_by(|a, b| numeric_cmp(b, a));
        versions.dedup();
        Ok(versions)
    }
}

/// NeoForge versions encode the game version: `1.21.1` → `21.1.*`, `26.1` → `26.1.0.*`.
fn neoforge_prefix(minecraft: &str) -> String {
    let parts: Vec<&str> = minecraft.split('.').collect();
    if parts.first() == Some(&"1") {
        format!(
            "{}.{}.",
            parts.get(1).unwrap_or(&"0"),
            parts.get(2).unwrap_or(&"0")
        )
    } else {
        format!(
            "{}.{}.{}.",
            parts.first().unwrap_or(&"0"),
            parts.get(1).unwrap_or(&"0"),
            parts.get(2).unwrap_or(&"0")
        )
    }
}

fn numeric_cmp(a: &str, b: &str) -> Ordering {
    let nums = |s: &str| -> Vec<u64> {
        s.split(['.', '-', '+'])
            .map_while(|p| p.parse().ok())
            .collect()
    };
    nums(a).cmp(&nums(b))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::*;

    fn offline(name: &str) -> GameMeta {
        let dir =
            std::env::temp_dir().join(format!("riven-test-game-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::new(dir, Duration::from_secs(3600));
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/game");
        let read = |f: &str| std::fs::read_to_string(fixtures.join(f)).unwrap();
        let manifest = read("version_manifest_v2.json");
        let url_1211 = serde_json::from_str::<Manifest>(&manifest)
            .unwrap()
            .versions
            .into_iter()
            .find(|v| v.id == "1.21.1")
            .unwrap()
            .url;
        for (url, file) in [
            (MOJANG_MANIFEST.to_owned(), "version_manifest_v2.json"),
            (url_1211, "1.21.1.json"),
            (format!("{FABRIC_META}/1.21.1"), "fabric-loader-1.21.1.json"),
            (format!("{QUILT_META}/1.21.1"), "quilt-loader-1.21.1.json"),
            (NEOFORGE_VERSIONS.to_owned(), "neoforge-versions.json"),
            (FORGE_PROMOTIONS.to_owned(), "forge-promotions.json"),
        ] {
            cache.put(&url, &read(file));
        }
        GameMeta::new(crate::client()).with_cache(cache)
    }

    #[tokio::test]
    async fn picks_stable_loader_versions() {
        let game = offline("loaders");
        let version = |loader, mc: &'static str| {
            let game = game.clone();
            async move { game.loader_version(loader, mc).await.unwrap() }
        };
        assert_eq!(version(LoaderKind::Fabric, "1.21.1").await, "0.19.5");
        assert_eq!(version(LoaderKind::Quilt, "1.21.1").await, "0.24.0");
        assert_eq!(version(LoaderKind::NeoForge, "1.21.1").await, "21.1.256");
        assert_eq!(version(LoaderKind::NeoForge, "26.1.2").await, "26.1.2.114");
        assert_eq!(version(LoaderKind::Forge, "26.2").await, "65.1.0");
        assert!(
            game.loader_version(LoaderKind::NeoForge, "1.12.2")
                .await
                .is_err()
        );
        let fabric = game
            .loader_versions(LoaderKind::Fabric, "1.21.1")
            .await
            .unwrap();
        assert!(fabric.contains(&"0.19.5".to_owned()));
        assert!(fabric.windows(2).all(|w| numeric_cmp(&w[0], &w[1]).is_ge()));
        let neoforge = game
            .loader_versions(LoaderKind::NeoForge, "1.21.1")
            .await
            .unwrap();
        assert!(neoforge.iter().all(|v| v.starts_with("21.1.")));
    }

    #[tokio::test]
    async fn minecraft_and_java() {
        let game = offline("minecraft");
        assert_eq!(game.latest_minecraft().await.unwrap(), "26.3");
        assert_eq!(game.java_major("1.21.1").await.unwrap(), 21);
        assert!(matches!(
            game.java_major("9.9").await,
            Err(Error::NotFound(_))
        ));
    }
}
