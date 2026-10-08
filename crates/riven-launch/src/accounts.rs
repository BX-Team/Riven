use std::path::PathBuf;

use md5::{Digest, Md5};
use riven_format::{Account, AccountKind, Accounts};
use riven_game::Session;
use serde::Deserialize;

use crate::LaunchError;
use crate::vault::Vault;

/// The "Riven Launcher" app registration in Microsoft Entra; a public client, so no secret.
const CLIENT_ID: &str = "f22b4c7a-4c72-4023-a747-0bc776d22ac1";
const SCOPE: &str = "XboxLive.signin offline_access";
const FORM: &str = "application/x-www-form-urlencoded";
const MS_OAUTH: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const XBL_AUTH: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE: &str = "https://api.minecraftservices.com/minecraft/profile";

/// What the player enters on Microsoft's page to let the launcher sign in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub code: String,
    pub url: String,
}

pub fn path() -> Result<PathBuf, LaunchError> {
    crate::config_file("accounts.json")
}

pub fn load() -> Result<Accounts, LaunchError> {
    crate::load_or_default(&path()?)
}

pub fn save(accounts: &Accounts) -> Result<(), LaunchError> {
    crate::save(&path()?, accounts)
}

/// The UUID the vanilla server gives an offline player: `nameUUIDFromBytes("OfflinePlayer:" + name)`.
pub fn offline_uuid(name: &str) -> String {
    let mut bytes: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// An offline account; the name must be a valid Minecraft player name.
pub fn offline(name: &str) -> Result<Account, LaunchError> {
    let valid = (3..=16).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(LaunchError::BadName(name.to_owned()));
    }
    Ok(Account {
        id: offline_uuid(name),
        name: name.to_owned(),
        kind: AccountKind::Offline,
    })
}

/// Signs in with a Microsoft account through a device code and remembers it for later launches.
pub async fn sign_in(
    on_code: impl Fn(DeviceCode) + Send + Sync + 'static,
) -> Result<Account, LaunchError> {
    let http = riven_sources::client();
    let code: DeviceCodeResponse = post_form(
        &http,
        &format!("{MS_OAUTH}/devicecode"),
        &[("client_id", CLIENT_ID), ("scope", SCOPE)],
    )
    .await?;
    on_code(DeviceCode {
        code: code.user_code.clone(),
        url: code.verification_uri.clone(),
    });
    let tokens = poll(&http, &code).await?;
    let (session, refresh) = minecraft(&http, tokens).await?;
    let account = Account {
        id: session.uuid.clone(),
        name: session.name.clone(),
        kind: AccountKind::Microsoft,
    };
    remember(&account.id, refresh).await?;
    Ok(account)
}

/// A fresh game session for an account: Microsoft ones trade their saved refresh token for one.
pub async fn session(account: &Account) -> Result<Session, LaunchError> {
    if account.kind == AccountKind::Offline {
        return Ok(Session::offline(account.name.clone(), account.id.clone()));
    }
    let id = account.id.clone();
    let saved = blocking(move || Vault::open().map(|v| v.get(&id))).await?;
    let refresh = saved.ok_or_else(|| LaunchError::SignInAgain(account.name.clone()))?;
    let http = riven_sources::client();
    let response = http
        .post(format!("{MS_OAUTH}/token"))
        .header(reqwest::header::CONTENT_TYPE, FORM)
        .body(form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("scope", SCOPE),
        ]))
        .send()
        .await
        .map_err(network)?;
    if response.status() == reqwest::StatusCode::BAD_REQUEST {
        return Err(LaunchError::SignInAgain(account.name.clone()));
    }
    let tokens: MsTokens = json(response).await?;
    let (session, refresh) = minecraft(&http, tokens).await?;
    remember(&account.id, refresh).await?;
    Ok(session)
}

/// Drops what is saved for an account's sign-in.
pub async fn forget(id: &str) -> Result<(), LaunchError> {
    let id = id.to_owned();
    blocking(move || Vault::open()?.remove(&id)).await
}

/// The PNG of a player's current skin, by their UUID, from Mojang's session server.
pub async fn skin(uuid: &str) -> Result<Vec<u8>, LaunchError> {
    use base64::Engine as _;
    #[derive(serde::Deserialize)]
    struct Profile {
        properties: Vec<Property>,
    }
    #[derive(serde::Deserialize)]
    struct Property {
        name: String,
        value: String,
    }
    #[derive(serde::Deserialize)]
    struct Textures {
        textures: Kinds,
    }
    #[derive(serde::Deserialize)]
    struct Kinds {
        #[serde(rename = "SKIN")]
        skin: Option<Texture>,
    }
    #[derive(serde::Deserialize)]
    struct Texture {
        url: String,
    }
    let failed = |e: reqwest::Error| LaunchError::Download(e.to_string());
    let http = riven_sources::client();
    let url = format!(
        "https://sessionserver.mojang.com/session/minecraft/profile/{}",
        uuid.replace('-', "")
    );
    let profile: Profile = http
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?
        .json()
        .await
        .map_err(failed)?;
    let encoded = profile
        .properties
        .into_iter()
        .find(|p| p.name == "textures")
        .ok_or_else(|| LaunchError::Download("no textures in the profile".into()))?
        .value;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| LaunchError::Download(e.to_string()))?;
    let textures: Textures =
        serde_json::from_slice(&decoded).map_err(|e| LaunchError::Download(e.to_string()))?;
    let skin = textures
        .textures
        .skin
        .ok_or_else(|| LaunchError::Download("the player has no skin".into()))?;
    let bytes = http
        .get(skin.url.replace("http://", "https://"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?
        .bytes()
        .await
        .map_err(failed)?;
    Ok(bytes.to_vec())
}

/// Keeps the newest refresh token: Microsoft hands out a new one with every refresh.
async fn remember(id: &str, refresh: Option<String>) -> Result<(), LaunchError> {
    let Some(token) = refresh else {
        return Ok(());
    };
    let id = id.to_owned();
    blocking(move || Vault::open()?.set(&id, &token)).await
}

/// Keychains answer synchronously and may wait on a D-Bus round trip.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, LaunchError> + Send + 'static,
) -> Result<T, LaunchError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| LaunchError::Vault(e.to_string()))?
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default = "five")]
    interval: u64,
    expires_in: u64,
}

fn five() -> u64 {
    5
}

#[derive(Deserialize)]
struct MsTokens {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct OAuthError {
    error: String,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct XboxToken {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    claims: XboxClaims,
}

#[derive(Deserialize)]
struct XboxClaims {
    xui: Vec<XboxUser>,
}

#[derive(Deserialize)]
struct XboxUser {
    uhs: String,
}

#[derive(Deserialize)]
struct XboxError {
    #[serde(rename = "XErr")]
    code: u64,
}

#[derive(Deserialize)]
struct MinecraftToken {
    access_token: String,
}

#[derive(Deserialize)]
struct Profile {
    id: String,
    name: String,
}

/// Waits for the player to enter the code, at the pace Microsoft asks for.
async fn poll(http: &reqwest::Client, code: &DeviceCodeResponse) -> Result<MsTokens, LaunchError> {
    let mut interval = code.interval.max(1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(code.expires_in);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        if std::time::Instant::now() > deadline {
            return Err(LaunchError::SignIn("the code expired; try again".into()));
        }
        let response = http
            .post(format!("{MS_OAUTH}/token"))
            .header(reqwest::header::CONTENT_TYPE, FORM)
            .body(form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", code.device_code.as_str()),
            ]))
            .send()
            .await
            .map_err(network)?;
        if response.status().is_success() {
            return json(response).await;
        }
        let error: OAuthError = json(response).await?;
        match error.error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval += 5,
            "authorization_declined" => {
                return Err(LaunchError::SignIn("sign-in was declined".into()));
            }
            "expired_token" => {
                return Err(LaunchError::SignIn("the code expired; try again".into()));
            }
            _ => {
                return Err(LaunchError::SignIn(
                    error.error_description.unwrap_or(error.error),
                ));
            }
        }
    }
}

/// Microsoft token → Xbox Live → XSTS → Minecraft token and profile; also returns the new refresh token.
async fn minecraft(
    http: &reqwest::Client,
    ms: MsTokens,
) -> Result<(Session, Option<String>), LaunchError> {
    let xbl: XboxToken = post_json(
        http,
        XBL_AUTH,
        &serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={}", ms.access_token),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }),
    )
    .await?;
    let response = http
        .post(XSTS_AUTH)
        .json(&serde_json::json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl.token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }))
        .send()
        .await
        .map_err(network)?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        let error: XboxError = json(response).await?;
        return Err(LaunchError::SignIn(xbox_error(error.code)));
    }
    let xsts: XboxToken = json(response).await?;
    let uhs = xsts
        .claims
        .xui
        .first()
        .map(|u| u.uhs.clone())
        .ok_or_else(|| LaunchError::SignIn("Xbox Live sent no user hash".into()))?;
    let response = http
        .post(MC_LOGIN)
        .json(&serde_json::json!({ "identityToken": format!("XBL3.0 x={uhs};{}", xsts.token) }))
        .send()
        .await
        .map_err(network)?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        let body = response.text().await.unwrap_or_default();
        return Err(LaunchError::SignIn(
            if body.contains("Invalid app registration") {
                "Microsoft has not approved this launcher for Minecraft yet".into()
            } else {
                "Minecraft refused the sign-in".into()
            },
        ));
    }
    let mc: MinecraftToken = json(response).await?;
    let response = http
        .get(MC_PROFILE)
        .bearer_auth(&mc.access_token)
        .send()
        .await
        .map_err(network)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(LaunchError::SignIn(
            "this Microsoft account has no Minecraft: Java Edition profile".into(),
        ));
    }
    let profile: Profile = json(response).await?;
    let session = Session {
        name: profile.name,
        uuid: dashed(&profile.id),
        xuid: xuid(&mc.access_token).unwrap_or_else(|| "0".into()),
        access_token: mc.access_token,
        user_type: "msa".into(),
        client_id: CLIENT_ID.into(),
    };
    Ok((session, ms.refresh_token))
}

fn xbox_error(code: u64) -> String {
    match code {
        2_148_916_233 => {
            "this Microsoft account has no Xbox profile; sign in at xbox.com once".into()
        }
        2_148_916_235 => "Xbox Live is not available in this account's country".into(),
        2_148_916_236 | 2_148_916_237 => "this account needs adult verification on xbox.com".into(),
        2_148_916_238 => {
            "this is a child account; an adult has to add it to a Microsoft family".into()
        }
        code => format!("Xbox Live refused the sign-in ({code})"),
    }
}

/// The Xbox user id inside the Minecraft token, a JWT; only for telemetry, so missing is fine.
fn xuid(token: &str) -> Option<String> {
    use base64::Engine as _;
    #[derive(Deserialize)]
    struct Claims {
        xuid: Option<String>,
    }
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice::<Claims>(&bytes).ok()?.xuid
}

/// `0123…` as `01234567-89ab-…`, the form offline UUIDs have too.
fn dashed(id: &str) -> String {
    if id.len() != 32 {
        return id.to_owned();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &id[0..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..32]
    )
}

async fn post_form<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<T, LaunchError> {
    let request = http
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, FORM)
        .body(self::form(form));
    json(request.send().await.map_err(network)?).await
}

fn form(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

async fn post_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
) -> Result<T, LaunchError> {
    json(http.post(url).json(body).send().await.map_err(network)?).await
}

/// Reads a JSON answer; the body of a failed request becomes the error.
async fn json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, LaunchError> {
    let status = response.status();
    let body = response.bytes().await.map_err(network)?;
    if !status.is_success() && serde_json::from_slice::<T>(&body).is_err() {
        return Err(LaunchError::SignIn(format!(
            "Microsoft answered HTTP {}",
            status.as_u16()
        )));
    }
    serde_json::from_slice(&body).map_err(|e| LaunchError::SignIn(e.to_string()))
}

fn network(e: reqwest::Error) -> LaunchError {
    LaunchError::SignIn(e.without_url().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuids_match_the_vanilla_server() {
        assert_eq!(
            offline_uuid("Notch"),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
        assert!(offline("Steve_1").is_ok());
        assert!(offline("ab").is_err());
        assert!(offline("Игрок").is_err());
    }
}
