use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use crate::lobby::Lobby;
use crate::{Activity, PlayUser, Presence, Status, base_url, id};

/// The server drops a socket it hears nothing on; this keeps NATs and proxies from doing the same.
const KEEPALIVE: Duration = Duration::from_secs(30);
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

/// What the play service pushes to a connected launcher.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Presence {
        #[serde(rename = "userId")]
        user_id: String,
        presence: Presence,
    },
    /// This account's own status: on connect and when another of its launchers changes it.
    Status {
        status: Status,
    },
    FriendRequest {
        user: PlayUser,
    },
    FriendAdded {
        user: PlayUser,
    },
    FriendRemoved {
        #[serde(rename = "userId")]
        user_id: String,
    },
    /// The player, or one of their friends, changed their skin.
    Skin {
        #[serde(rename = "userId")]
        user_id: String,
    },
    Invite {
        lobby: Box<Lobby>,
        from: PlayUser,
    },
    Lobby {
        lobby: Box<Lobby>,
    },
    LobbyClosed {
        lobby: String,
    },
    Signal {
        lobby: String,
        from: String,
        data: serde_json::Value,
    },
    Pong,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Activity {
        activity: Option<Activity>,
    },
    Status {
        status: Status,
    },
    Signal {
        lobby: String,
        to: String,
        data: serde_json::Value,
    },
    Ping,
}

/// What the connection reports besides server events.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Connected,
    /// The socket dropped; it reconnects by itself.
    Disconnected,
    /// BX ID no longer accepts the session; the connection stops.
    SignedOut,
    Server(ServerEvent),
}

/// A live presence connection; dropping it closes the socket and stops reconnecting.
pub struct Connection {
    activity: watch::Sender<Option<Activity>>,
    outbox: mpsc::UnboundedSender<ClientMessage>,
}

impl Connection {
    /// Tells friends what this player is doing; also sent again after every reconnect.
    pub fn set_activity(&self, activity: Option<Activity>) {
        self.activity.send_replace(activity);
    }

    /// Shows friends this status; the server keeps it for the account's next connections.
    pub fn set_status(&self, status: Status) {
        let _ = self.outbox.send(ClientMessage::Status { status });
    }

    /// A handle that passes connection offers to other members of a lobby.
    pub fn signaller(&self) -> Signaller {
        Signaller(self.outbox.clone())
    }
}

#[derive(Clone)]
pub struct Signaller(mpsc::UnboundedSender<ClientMessage>);

impl Signaller {
    /// Queued while the socket is down and sent once it is back.
    pub fn send(&self, lobby: &str, to: &str, data: serde_json::Value) {
        let _ = self.0.send(ClientMessage::Signal {
            lobby: lobby.to_owned(),
            to: to.to_owned(),
            data,
        });
    }
}

/// Connects on the current tokio runtime; `on_event` runs on that runtime for every event.
pub fn connect(on_event: impl Fn(Event) + Send + Sync + 'static) -> Connection {
    let (activity, mut activity_rx) = watch::channel(None);
    let (outbox, mut outbox_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut retry = RETRY_MIN;
        loop {
            match session(&on_event, &mut activity_rx, &mut outbox_rx).await {
                Ended::Dropped => return,
                Ended::SignedOut => {
                    on_event(Event::SignedOut);
                    return;
                }
                Ended::Lost { was_open } => {
                    if was_open {
                        on_event(Event::Disconnected);
                        retry = RETRY_MIN;
                    }
                }
            }
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(RETRY_MAX);
            if outbox_rx.is_closed() {
                return;
            }
        }
    });
    Connection { activity, outbox }
}

enum Ended {
    Dropped,
    SignedOut,
    Lost { was_open: bool },
}

async fn session(
    on_event: &(impl Fn(Event) + Send + Sync),
    activity: &mut watch::Receiver<Option<Activity>>,
    outbox: &mut mpsc::UnboundedReceiver<ClientMessage>,
) -> Ended {
    let token = match id::session_token().await {
        Ok(Some(token)) => token,
        Ok(None) => return Ended::SignedOut,
        Err(e) => {
            tracing::debug!("presence: {e}");
            return Ended::Lost { was_open: false };
        }
    };
    let url = format!("{}/v1/play/connect", ws_base(&base_url()));
    let Ok(mut request) = url.as_str().into_client_request() else {
        tracing::debug!("presence: bad URL {url}");
        return Ended::Lost { was_open: false };
    };
    let Ok(bearer) = format!("Bearer {token}").parse() else {
        return Ended::SignedOut;
    };
    request.headers_mut().insert("Authorization", bearer);
    let mut socket = match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _)) => socket,
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status() == 401 => {
            return Ended::SignedOut;
        }
        Err(e) => {
            tracing::debug!("presence: cannot connect: {e}");
            return Ended::Lost { was_open: false };
        }
    };
    on_event(Event::Connected);

    let current = ClientMessage::Activity {
        activity: activity.borrow_and_update().clone(),
    };
    if send(&mut socket, &current).await.is_err() {
        return Ended::Lost { was_open: true };
    }
    let mut keepalive = tokio::time::interval(KEEPALIVE);
    keepalive.tick().await;
    loop {
        tokio::select! {
            changed = activity.changed() => {
                if changed.is_err() {
                    return Ended::Dropped;
                }
                let message = ClientMessage::Activity { activity: activity.borrow_and_update().clone() };
                if send(&mut socket, &message).await.is_err() {
                    return Ended::Lost { was_open: true };
                }
            }
            message = outbox.recv() => {
                let Some(message) = message else { return Ended::Dropped };
                if send(&mut socket, &message).await.is_err() {
                    return Ended::Lost { was_open: true };
                }
            }
            _ = keepalive.tick() => {
                if send(&mut socket, &ClientMessage::Ping).await.is_err() {
                    return Ended::Lost { was_open: true };
                }
            }
            frame = socket.next() => match frame {
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ServerEvent>(&text) {
                    Ok(ServerEvent::Pong) => {}
                    Ok(event) => on_event(Event::Server(event)),
                    Err(e) => tracing::debug!("presence: unknown event: {e}"),
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Ended::Lost { was_open: true },
                Some(Ok(_)) => {}
            },
        }
    }
}

async fn send<S>(socket: &mut S, message: &ClientMessage) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let text = serde_json::to_string(message).map_err(|_| ())?;
    socket.send(Message::text(text)).await.map_err(|_| ())
}

fn ws_base(http: &str) -> String {
    if let Some(rest) = http.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = http.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        http.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_events_read_the_wire_names() {
        let presence = r#"{"type":"presence","userId":"ub","presence":{"online":true,"status":"busy","activity":{"name":"Cozy Valley","minecraft":"1.21.4"}}}"#;
        assert_eq!(
            serde_json::from_str::<ServerEvent>(presence).unwrap(),
            ServerEvent::Presence {
                user_id: "ub".into(),
                presence: Presence {
                    online: true,
                    status: Status::Busy,
                    activity: Some(Activity {
                        name: "Cozy Valley".into(),
                        minecraft: "1.21.4".into(),
                        lobby: None,
                        players: 0,
                    }),
                    last_seen: None,
                },
            }
        );
        let away = r#"{"online":false,"activity":null,"lastSeen":"2026-11-24T10:00:00Z"}"#;
        let away = serde_json::from_str::<Presence>(away).unwrap();
        assert_eq!(away.status, Status::Online);
        assert_eq!(away.last_seen.as_deref(), Some("2026-11-24T10:00:00Z"));
        let removed = r#"{"type":"friend_removed","userId":"ua"}"#;
        assert_eq!(
            serde_json::from_str::<ServerEvent>(removed).unwrap(),
            ServerEvent::FriendRemoved {
                user_id: "ua".into()
            }
        );
    }
}
