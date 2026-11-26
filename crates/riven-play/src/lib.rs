pub mod friends;
pub mod id;
pub mod lan;
pub mod lobby;
pub mod net;
pub mod presence;
pub mod skins;
pub mod tunnel;

pub use friends::{Activity, Friend, FriendAdded, Friends, PlayUser, Presence, Status};

const DEFAULT_BASE: &str = "https://api.bxteam.org";

#[derive(Debug, thiserror::Error)]
pub enum PlayError {
    #[error("not signed in to BX ID")]
    SignedOut,
    #[error("the BX ID sign-in code expired; start again")]
    Expired,
    #[error("the BX ID sign-in was denied")]
    Denied,
    #[error("cannot reach BX Team: {0}")]
    Network(String),
    #[error("{0}")]
    Refused(Problem),
    /// What was asked for is gone or was never there, as HTTP 404.
    #[error("{0}")]
    NotFound(Problem),
    /// Someone else holds it, as HTTP 409: a nick claimed by another BX ID.
    #[error("{0}")]
    Taken(Problem),
    #[error("BX Team sent an answer Riven cannot read: {0}")]
    Decode(String),
    #[error("cannot connect to the other player: {0}")]
    Connect(String),
    #[error(transparent)]
    Launch(#[from] riven_launch::LaunchError),
}

/// Why BX Team refused: a stable `code` clients translate, its `params`, and an English `message`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct Problem {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

impl Problem {
    /// A refusal that came without a code, such as an OAuth error.
    pub fn text(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Self::default()
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl PlayError {
    /// What BX Team said, when it said anything.
    pub fn problem(&self) -> Option<&Problem> {
        match self {
            Self::Refused(p) | Self::NotFound(p) | Self::Taken(p) => Some(p),
            _ => None,
        }
    }
}

/// The origin of BX ID and play; `RIVEN_BXID_URL` points a dev build at `wrangler dev`.
pub fn base_url() -> String {
    std::env::var("RIVEN_BXID_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_owned())
        .trim_end_matches('/')
        .to_owned()
}

/// A signed-in request to the play service at `path` under `/v1/play`.
async fn send(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<reqwest::Response, PlayError> {
    let token = id::session_token().await?.ok_or(PlayError::SignedOut)?;
    let mut request = riven_sources::client()
        .request(method, format!("{}/v1/play{path}", base_url()))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    request.send().await.map_err(network)
}

fn network(e: reqwest::Error) -> PlayError {
    PlayError::Network(e.without_url().to_string())
}

/// Reads a JSON answer; an error status becomes the server's [`Problem`] when it sent one.
async fn read<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T, PlayError> {
    let status = response.status();
    let body = response.bytes().await.map_err(network)?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(PlayError::SignedOut);
    }
    if !status.is_success() {
        let mut problem = serde_json::from_slice::<Problem>(&body).unwrap_or_default();
        if problem.message.is_empty() {
            problem.message = format!("BX Team answered HTTP {}", status.as_u16());
        }
        return Err(match status {
            reqwest::StatusCode::NOT_FOUND => PlayError::NotFound(problem),
            reqwest::StatusCode::CONFLICT => PlayError::Taken(problem),
            _ => PlayError::Refused(problem),
        });
    }
    serde_json::from_slice(&body).map_err(|e| PlayError::Decode(e.to_string()))
}

/// An answer with no body worth reading, like 204; errors are handled as in [`read`].
async fn done(response: reqwest::Response) -> Result<(), PlayError> {
    if response.status().is_success() {
        return Ok(());
    }
    read::<serde_json::Value>(response).await.map(|_| ())
}
