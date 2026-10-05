use crate::version::VersionJson;
use crate::{Dirs, Error, GameVersion, Loader, net};

const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions/loader";
const QUILT_META: &str = "https://meta.quiltmc.org/v3/versions/loader";

/// The loader's launcher profile, which inherits from vanilla; builds never change, so it is fetched once.
pub async fn profile(
    http: &reqwest::Client,
    dirs: &Dirs,
    game: &GameVersion,
) -> Result<VersionJson, Error> {
    let (meta, prefix, name) = match game.loader {
        Loader::Quilt => (QUILT_META, "quilt-loader", "Quilt"),
        _ => (FABRIC_META, "fabric-loader", "Fabric"),
    };
    let (mc, version) = (&game.minecraft, &game.loader_version);
    let id = format!("{prefix}-{version}-{mc}");
    let path = dirs.versions().join(&id).join(format!("{id}.json"));
    if let Ok(bytes) = std::fs::read(&path) {
        return crate::json(&id, &bytes);
    }
    let url = format!("{meta}/{mc}/{version}/profile/json");
    let bytes = match net::get(http, &url).await {
        Err(Error::Status {
            status: 400..=499, ..
        }) => {
            return Err(Error::NoLoader(format!(
                "{name} {version} for Minecraft {mc}"
            )));
        }
        other => other?,
    };
    let profile = crate::json(&url, &bytes)?;
    net::save(&path, &bytes)?;
    Ok(profile)
}
