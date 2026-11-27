use serde::{Deserialize, Serialize};
use serde_json::json;

use riven_launch::agent::SkinsAgent;

use crate::{PlayError, done, id, network, read};

/// Arms of the model: Steve's 4 pixels or Alex's 3.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkinModel {
    #[default]
    Classic,
    Slim,
}

impl SkinModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Slim => "slim",
        }
    }
}

/// A skin a nick wears; `url` never changes its content, so it can be cached by `hash`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skin {
    pub model: SkinModel,
    /// SHA-256 of the PNG, hex.
    pub hash: String,
    pub url: String,
}

/// The skins this BX ID uploaded, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MySkins {
    #[serde(default)]
    pub recent: Vec<Skin>,
}

/// An offline nick this BX ID holds, with the skin it wears.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Profile {
    /// The offline UUID, undashed.
    pub id: String,
    pub name: String,
    pub skin: Option<Skin>,
}

/// A nick this BX ID removed and may take back until `until`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HeldNick {
    pub name: String,
    pub until: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MyProfiles {
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub held: Vec<HeldNick>,
    /// How many nicks one BX ID may hold.
    pub limit: usize,
}

impl MyProfiles {
    /// The nick, matched without regard to case as the service does.
    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// The skins service; `RIVEN_SKINS_URL` points a dev build at its own `wrangler dev`, else it shares BX ID's host.
pub fn base_url() -> String {
    std::env::var("RIVEN_SKINS_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| v.trim_end_matches('/').to_owned())
        .unwrap_or_else(crate::base_url)
}

fn url(path: &str) -> String {
    format!("{}/v1/skins{path}", base_url())
}

async fn signed(request: reqwest::RequestBuilder) -> Result<reqwest::Response, PlayError> {
    let token = id::session_token().await?.ok_or(PlayError::SignedOut)?;
    request.bearer_auth(token).send().await.map_err(network)
}

/// The library: every skin this BX ID uploaded.
pub async fn mine() -> Result<MySkins, PlayError> {
    read(signed(riven_sources::client().get(url("/me"))).await?).await
}

pub async fn profiles() -> Result<MyProfiles, PlayError> {
    read(signed(riven_sources::client().get(url("/profiles"))).await?).await
}

/// Claims a nick for this BX ID, or takes back one it released; [`PlayError::Taken`] when another holds it.
pub async fn claim(name: &str) -> Result<Profile, PlayError> {
    let request = riven_sources::client()
        .post(url("/profiles"))
        .json(&json!({ "name": name }));
    read(signed(request).await?).await
}

/// Lets go of a nick; for a few days only this BX ID can take it back.
pub async fn release(name: &str) -> Result<(), PlayError> {
    done(signed(riven_sources::client().delete(url(&format!("/profiles/{name}")))).await?).await
}

/// Puts a PNG on a nick; the service checks that it is a 64×64 or 64×32 skin.
pub async fn upload(name: &str, png: Vec<u8>, model: SkinModel) -> Result<Profile, PlayError> {
    let request = riven_sources::client()
        .put(url(&format!(
            "/profiles/{name}/skin?model={}",
            model.as_str()
        )))
        .header(reqwest::header::CONTENT_TYPE, "image/png")
        .body(png);
    read(signed(request).await?).await
}

/// Takes the nick's skin off; the game shows the default one.
pub async fn reset(name: &str) -> Result<(), PlayError> {
    done(signed(riven_sources::client().delete(url(&format!("/profiles/{name}/skin")))).await?)
        .await
}

/// The PNG behind a skin's URL.
pub async fn download(url: &str) -> Result<Vec<u8>, PlayError> {
    let response = riven_sources::client()
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(network)?;
    Ok(response.bytes().await.map_err(network)?.to_vec())
}

/// What the game's skins agent needs. The game token comes only for an offline account named after one of
/// the signed-in BX ID's nicks; without it the game still shows Riven skins but joins as before.
pub async fn agent(offline_name: Option<&str>) -> Result<SkinsAgent, PlayError> {
    #[derive(Deserialize)]
    struct Key {
        key: String,
    }
    #[derive(Deserialize)]
    struct Token {
        token: String,
    }
    let http = riven_sources::client();
    let key: Key = read(
        http.get(url("/session/publickey"))
            .send()
            .await
            .map_err(network)?,
    )
    .await?;
    let game_token = match offline_name {
        Some(name) => {
            let request = http
                .post(url("/session/token"))
                .json(&json!({ "name": name }));
            match signed(request).await {
                Ok(response) => match read::<Token>(response).await {
                    Ok(token) => Some(token.token),
                    Err(PlayError::NotFound(_) | PlayError::SignedOut) => None,
                    Err(e) => return Err(e),
                },
                Err(PlayError::SignedOut) => None,
                Err(e) => return Err(e),
            }
        }
        None => None,
    };
    Ok(SkinsAgent {
        api: url(""),
        key: key.key,
        game_token,
    })
}
