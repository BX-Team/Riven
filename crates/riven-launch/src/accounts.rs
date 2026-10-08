use std::path::PathBuf;

use lighty_launcher::auth::MicrosoftAuth;
use lighty_launcher::auth::{AuthError, AuthProvider, Authenticator as _, ExposeSecret as _};
use lighty_launcher::auth::{SecretString, UserProfile};
use md5::{Digest, Md5};
use riven_format::{Account, AccountKind, Accounts};

use crate::LaunchError;
use crate::vault::Vault;

/// The "Riven Launcher" app registration in Microsoft Entra; a public client, so no secret.
const CLIENT_ID: &str = "f22b4c7a-4c72-4023-a747-0bc776d22ac1";

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
    let mut auth = MicrosoftAuth::new(CLIENT_ID);
    auth.set_device_code_callback(move |code, url| {
        on_code(DeviceCode {
            code: code.to_owned(),
            url: url.to_owned(),
        })
    });
    let profile = auth.authenticate(None).await.map_err(auth_error)?;
    let account = Account {
        id: profile.uuid.clone(),
        name: profile.username.clone(),
        kind: AccountKind::Microsoft,
    };
    remember(&account.id, &profile).await?;
    Ok(account)
}

/// A fresh game session for an account: Microsoft ones trade their saved refresh token for one.
pub async fn session(account: &Account) -> Result<UserProfile, LaunchError> {
    if account.kind == AccountKind::Offline {
        return Ok(UserProfile::offline(
            account.name.clone(),
            account.id.clone(),
        ));
    }
    let id = account.id.clone();
    let saved = blocking(move || Vault::open().map(|v| v.get(&id))).await?;
    let refresh = saved.ok_or_else(|| LaunchError::SignInAgain(account.name.clone()))?;
    let mut auth = MicrosoftAuth::new(CLIENT_ID);
    let profile = auth
        .authenticate_with_refresh_token(&SecretString::from(refresh), None)
        .await
        .map_err(|e| match e {
            AuthError::InvalidToken => LaunchError::SignInAgain(account.name.clone()),
            e => auth_error(e),
        })?;
    remember(&account.id, &profile).await?;
    Ok(profile)
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
async fn remember(id: &str, profile: &UserProfile) -> Result<(), LaunchError> {
    let AuthProvider::Microsoft {
        refresh_token: Some(token),
        ..
    } = &profile.provider
    else {
        return Ok(());
    };
    let id = id.to_owned();
    let token = token.expose_secret().to_owned();
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

fn auth_error(e: AuthError) -> LaunchError {
    match e {
        AuthError::HttpStatus { status: 403, body }
            if body.contains("Invalid app registration") =>
        {
            LaunchError::SignIn("Microsoft has not approved this launcher for Minecraft yet".into())
        }
        AuthError::Cancelled => LaunchError::SignIn("sign-in was declined".into()),
        AuthError::DeviceCodeExpired => LaunchError::SignIn("the code expired; try again".into()),
        AuthError::HttpStatus { status, .. } => {
            LaunchError::SignIn(format!("Microsoft answered HTTP {status}"))
        }
        e => LaunchError::SignIn(e.to_string()),
    }
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
