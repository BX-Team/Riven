use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{PlayError, done, read, send};

/// A BX ID as other players see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayUser {
    pub id: String,
    /// The display name; empty when the player set none.
    #[serde(default)]
    pub name: String,
    pub username: Option<String>,
    pub image: Option<String>,
}

impl PlayUser {
    /// The display name, else the username.
    pub fn shown(&self) -> &str {
        Some(self.name.as_str())
            .filter(|n| !n.trim().is_empty())
            .or(self.username.as_deref())
            .unwrap_or("?")
    }

    /// `@username`, when the name shown is not already it.
    pub fn handle(&self) -> Option<String> {
        handle(self.shown(), self.username.as_deref())
    }
}

/// `@username` beside a display name, left out when the two read the same.
pub fn handle(shown: &str, username: Option<&str>) -> Option<String> {
    username
        .filter(|u| !u.eq_ignore_ascii_case(shown))
        .map(|u| format!("@{u}"))
}

/// What a player is doing in the launcher, shown to their friends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    /// The instance or pack name.
    pub name: String,
    pub minecraft: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lobby: Option<String>,
    /// Players in the open lobby, the host included; 0 without one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub players: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence {
    pub online: bool,
    /// `Busy` while the friend asks not to be disturbed; never `Invisible`, which reads as offline.
    #[serde(default)]
    pub status: Status,
    pub activity: Option<Activity>,
    /// RFC 3339, while offline.
    #[serde(default)]
    pub last_seen: Option<String>,
}

/// What a player shows their friends; `Invisible` looks offline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Online,
    Busy,
    Invisible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Friend {
    pub user: PlayUser,
    pub presence: Presence,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Friends {
    pub friends: Vec<Friend>,
    pub incoming: Vec<PlayUser>,
    pub outgoing: Vec<PlayUser>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendAdded {
    pub status: AddStatus,
    pub user: PlayUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddStatus {
    Requested,
    /// They had already asked; now both are friends.
    Accepted,
    Exists,
}

pub async fn list() -> Result<Friends, PlayError> {
    read(send(reqwest::Method::GET, "/friends", None).await?).await
}

/// Asks the owner of a friend code, as typed: the server ignores dashes, spaces and case.
pub async fn add(code: &str) -> Result<FriendAdded, PlayError> {
    let body = json!({ "code": code });
    read(send(reqwest::Method::POST, "/friends", Some(body)).await?).await
}

pub async fn accept(user_id: &str) -> Result<(), PlayError> {
    done(
        send(
            reqwest::Method::POST,
            &format!("/friends/{user_id}/accept"),
            None,
        )
        .await?,
    )
    .await
}

/// Removes a friend, declines their request or cancels one sent to them.
pub async fn remove(user_id: &str) -> Result<(), PlayError> {
    done(
        send(
            reqwest::Method::DELETE,
            &format!("/friends/{user_id}"),
            None,
        )
        .await?,
    )
    .await
}
