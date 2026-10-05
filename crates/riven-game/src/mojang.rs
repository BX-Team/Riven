use serde::Deserialize;

use crate::version::VersionJson;
use crate::{Dirs, Error, net};

const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Deserialize)]
struct Manifest {
    versions: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    url: String,
    sha1: String,
}

/// The vanilla version file, kept in `versions/<id>/<id>.json` and refreshed when Mojang changes it.
pub async fn version(http: &reqwest::Client, dirs: &Dirs, id: &str) -> Result<VersionJson, Error> {
    let path = dirs.versions().join(id).join(format!("{id}.json"));
    let saved = std::fs::read(&path).ok();
    match net::get_json::<Manifest>(http, MANIFEST).await {
        Ok(manifest) => {
            let entry = manifest
                .versions
                .into_iter()
                .find(|v| v.id == id)
                .ok_or_else(|| Error::NoMinecraft(id.to_owned()))?;
            let current = saved
                .as_deref()
                .is_some_and(|b| net::sha1_bytes(b).eq_ignore_ascii_case(&entry.sha1));
            if !current {
                let bytes = net::get(http, &entry.url).await?;
                if !net::sha1_bytes(&bytes).eq_ignore_ascii_case(&entry.sha1) {
                    return Err(Error::Checksum { url: entry.url });
                }
                net::save(&path, &bytes)?;
                return crate::json(&entry.url, &bytes);
            }
        }
        Err(e) if saved.is_some() => {
            tracing::warn!("using the saved metadata of Minecraft {id}: {e}");
        }
        Err(e) => return Err(e),
    }
    crate::json(
        &path.display().to_string(),
        saved.as_deref().unwrap_or_default(),
    )
}
