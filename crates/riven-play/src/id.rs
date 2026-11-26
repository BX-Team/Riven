use std::time::{Duration, Instant};

use riven_launch::vault;
use serde::Deserialize;
use serde_json::json;

use crate::{PlayError, Problem, base_url, network, read};

/// The device-flow client BX ID knows Riven by.
const CLIENT_ID: &str = "riven";
/// Where the session token sits in Riven's vault, next to the Microsoft refresh tokens.
const SECRET: &str = "bxid";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// What the player enters on bxteam.org to let this launcher in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// The code as shown on the site, `ABCD-EFGH`.
    pub code: String,
    /// The device page with the code filled in.
    pub url: String,
}

/// The signed-in BX ID.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    pub id: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_username: Option<String>,
    /// What a friend types to add this player; it stays when the username changes.
    #[serde(default)]
    pub friend_code: Option<String>,
}

impl Me {
    /// The name to show: the display name, else the username as typed.
    pub fn shown(&self) -> &str {
        Some(self.name.as_str())
            .filter(|n| !n.trim().is_empty())
            .or(self.display_username.as_deref())
            .or(self.username.as_deref())
            .unwrap_or(&self.email)
    }

    /// `@username`, when the name shown is not already it.
    pub fn handle(&self) -> Option<String> {
        let username = self
            .display_username
            .as_deref()
            .or(self.username.as_deref());
        crate::friends::handle(self.shown(), username)
    }

    /// The friend code as people read it, `K7F2-9QXM`.
    pub fn shown_code(&self) -> Option<String> {
        let code = self.friend_code.as_deref()?;
        Some(match code.split_at_checked(4) {
            Some((head, tail)) if !tail.is_empty() => format!("{head}-{tail}"),
            _ => code.to_owned(),
        })
    }
}

/// Signs in through the device flow: `on_code` shows the code, the player approves it on bxteam.org.
pub async fn sign_in(
    on_code: impl Fn(DeviceCode) + Send + Sync + 'static,
) -> Result<Me, PlayError> {
    #[derive(Deserialize)]
    struct Code {
        device_code: String,
        user_code: String,
        verification_uri_complete: String,
        interval: Option<u64>,
        expires_in: u64,
    }
    let http = riven_sources::client();
    let code: Code = read(
        http.post(format!("{}/v1/auth/device/code", base_url()))
            .json(&json!({ "client_id": CLIENT_ID }))
            .send()
            .await
            .map_err(network)?,
    )
    .await?;
    on_code(DeviceCode {
        code: dashed(&code.user_code),
        url: code.verification_uri_complete,
    });

    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = Duration::from_secs(code.interval.unwrap_or(5).max(1));
    let token = loop {
        tokio::time::sleep(interval).await;
        if Instant::now() > deadline {
            return Err(PlayError::Expired);
        }
        let response = http
            .post(format!("{}/v1/auth/device/token", base_url()))
            .json(&json!({
                "grant_type": DEVICE_GRANT,
                "device_code": code.device_code,
                "client_id": CLIENT_ID,
            }))
            .send()
            .await
            .map_err(network)?;
        match poll_answer(response.status(), &response.bytes().await.map_err(network)?)? {
            Poll::Token(token) => break token,
            Poll::Wait => {}
            Poll::SlowDown => interval += Duration::from_secs(5),
        }
    };

    sign_in_with_token(token).await
}

/// Signs in with a session token got elsewhere, such as a seeded dev account.
pub async fn sign_in_with_token(token: String) -> Result<Me, PlayError> {
    blocking(move || vault::set_secret(SECRET, &token)).await?;
    me().await?.ok_or(PlayError::SignedOut)
}

enum Poll {
    Token(String),
    Wait,
    SlowDown,
}

/// One answer of the token endpoint: RFC 8628 errors come as 400 with an `error` code.
fn poll_answer(status: reqwest::StatusCode, body: &[u8]) -> Result<Poll, PlayError> {
    #[derive(Deserialize)]
    struct Answer {
        access_token: Option<String>,
        error: Option<String>,
        error_description: Option<String>,
    }
    let answer: Answer =
        serde_json::from_slice(body).map_err(|e| PlayError::Decode(e.to_string()))?;
    if let (true, Some(token)) = (status.is_success(), answer.access_token) {
        return Ok(Poll::Token(token));
    }
    match answer.error.as_deref() {
        Some("authorization_pending") => Ok(Poll::Wait),
        Some("slow_down") => Ok(Poll::SlowDown),
        Some("expired_token") => Err(PlayError::Expired),
        Some("access_denied") => Err(PlayError::Denied),
        _ => Err(PlayError::Refused(Problem::text(
            answer
                .error_description
                .unwrap_or_else(|| format!("BX ID answered HTTP {}", status.as_u16())),
        ))),
    }
}

/// The signed-in account, or `None`; a session BX ID no longer accepts is forgotten.
pub async fn me() -> Result<Option<Me>, PlayError> {
    #[derive(Deserialize)]
    struct Current {
        user: Me,
    }
    let Some(token) = session_token().await? else {
        return Ok(None);
    };
    let response = riven_sources::client()
        .get(format!("{}/v1/auth/get-session", base_url()))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(network)?;
    match read::<Option<Current>>(response).await {
        Ok(Some(current)) => Ok(Some(current.user)),
        Ok(None) | Err(PlayError::SignedOut) => {
            forget().await?;
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Ends the session on BX ID and forgets it here, even when BX ID is unreachable.
pub async fn sign_out() -> Result<(), PlayError> {
    if let Some(token) = session_token().await? {
        let sent = riven_sources::client()
            .post(format!("{}/v1/auth/sign-out", base_url()))
            .bearer_auth(&token)
            .json(&json!({}))
            .send()
            .await;
        if let Err(e) = sent {
            tracing::warn!(
                "BX ID sign-out did not reach the server: {}",
                e.without_url()
            );
        }
    }
    forget().await
}

async fn forget() -> Result<(), PlayError> {
    blocking(|| vault::forget_secret(SECRET)).await
}

pub(crate) async fn session_token() -> Result<Option<String>, PlayError> {
    blocking(|| vault::secret(SECRET)).await
}

/// `ABCDEFGH` as `ABCD-EFGH`, the way people read a code aloud; BX ID ignores the dash.
fn dashed(code: &str) -> String {
    if code.len() == 8 && code.is_ascii() {
        format!("{}-{}", &code[..4], &code[4..])
    } else {
        code.to_owned()
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, riven_launch::LaunchError> + Send + 'static,
) -> Result<T, PlayError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| PlayError::Refused(Problem::text(e.to_string())))?
        .map_err(PlayError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_token_errors_follow_rfc_8628() {
        let bad = reqwest::StatusCode::BAD_REQUEST;
        let pending = br#"{"error":"authorization_pending","error_description":"waiting"}"#;
        assert!(matches!(poll_answer(bad, pending), Ok(Poll::Wait)));
        let slow = br#"{"error":"slow_down"}"#;
        assert!(matches!(poll_answer(bad, slow), Ok(Poll::SlowDown)));
        let denied = br#"{"error":"access_denied"}"#;
        assert!(matches!(poll_answer(bad, denied), Err(PlayError::Denied)));
        let ok = br#"{"access_token":"abc","token_type":"Bearer","expires_in":604800,"scope":""}"#;
        assert!(
            matches!(poll_answer(reqwest::StatusCode::OK, ok), Ok(Poll::Token(t)) if t == "abc")
        );
    }
}
