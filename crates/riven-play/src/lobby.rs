use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::net::IceServers;
use crate::{PlayError, done, read, send};

/// What guests need to join: the game version and, for a pack, where to install it from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyGame {
    pub minecraft: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
}

impl LobbyGame {
    pub fn of(instance: &riven_format::Instance) -> Self {
        Self {
            minecraft: instance.minecraft.clone(),
            loader: instance.loader.as_ref().map(|l| {
                let kind = serde_json::to_value(l.kind).ok();
                let kind = kind.as_ref().and_then(|k| k.as_str()).unwrap_or_default();
                format!("{kind} {}", l.version)
            }),
            pack: None,
        }
    }
}

/// A world a host opened to friends; members and invited players are BX ID ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lobby {
    pub id: String,
    pub name: String,
    pub host: String,
    pub game: LobbyGame,
    pub members: Vec<String>,
    pub invited: Vec<String>,
    /// Friends of the host may join without an invite.
    #[serde(default)]
    pub open: bool,
    pub created_at: String,
}

/// Opens a lobby hosted by this account; one it hosted before is closed.
/// An `open` lobby lets the host's friends join without an invite.
pub async fn create(name: &str, game: &LobbyGame, open: bool) -> Result<Lobby, PlayError> {
    let body = json!({ "name": name, "game": game, "open": open });
    read(send(Method::POST, "/lobbies", Some(body)).await?).await
}

pub async fn get(id: &str) -> Result<Lobby, PlayError> {
    read(send(Method::GET, &format!("/lobbies/{id}"), None).await?).await
}

/// Invites a friend; only the host can.
pub async fn invite(id: &str, user_id: &str) -> Result<Lobby, PlayError> {
    let body = json!({ "userId": user_id });
    read(send(Method::POST, &format!("/lobbies/{id}/invite"), Some(body)).await?).await
}

/// Sends a player out or takes their invite back; they come back only on a new invite. Only the host can.
pub async fn kick(id: &str, user_id: &str) -> Result<Lobby, PlayError> {
    let body = json!({ "userId": user_id });
    read(send(Method::POST, &format!("/lobbies/{id}/kick"), Some(body)).await?).await
}

/// Lets the host's friends join without an invite, or stops it; only the host can.
pub async fn set_open(id: &str, open: bool) -> Result<Lobby, PlayError> {
    let body = json!({ "open": open });
    read(send(Method::POST, &format!("/lobbies/{id}/open"), Some(body)).await?).await
}

pub async fn join(id: &str) -> Result<Lobby, PlayError> {
    read(send(Method::POST, &format!("/lobbies/{id}/join"), None).await?).await
}

/// Leaves a lobby; the host leaving closes it.
pub async fn leave(id: &str) -> Result<(), PlayError> {
    done(send(Method::POST, &format!("/lobbies/{id}/leave"), None).await?).await
}

pub async fn close(id: &str) -> Result<(), PlayError> {
    done(send(Method::DELETE, &format!("/lobbies/{id}"), None).await?).await
}

/// STUN and TURN servers with credentials good for a few hours.
pub async fn ice() -> Result<IceServers, PlayError> {
    read(send(Method::GET, "/ice", None).await?).await
}
