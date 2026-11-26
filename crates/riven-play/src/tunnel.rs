use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::lobby::{self, Lobby, LobbyGame};
use crate::net::tls::Fingerprint;
use crate::net::{Offer, Peer};
use crate::presence::Signaller;
use crate::{PlayError, lan};

/// The first byte of every stream says what it carries; a game connection is the only kind yet.
const GAME_STREAM: u8 = 1;
/// The close code of a connection the host cut because it sent the guest out.
const KICKED: u32 = 2;
const OFFER_AGAIN: Duration = Duration::from_secs(3);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Signal {
    Offer(Offer),
    Answer(Offer),
}

type Inboxes = HashMap<String, mpsc::UnboundedSender<(String, Value)>>;

/// Routes signals from the presence socket to the host or guest session of their lobby.
#[derive(Clone, Default)]
pub struct Signals(Arc<Mutex<Inboxes>>);

impl Signals {
    pub fn deliver(&self, lobby: &str, from: String, data: Value) {
        if let Some(inbox) = self.0.lock().expect("signals").get(lobby) {
            let _ = inbox.send((from, data));
        }
    }

    fn listen(&self, lobby: &str) -> Listening {
        let (tx, rx) = mpsc::unbounded_channel();
        self.0.lock().expect("signals").insert(lobby.to_owned(), tx);
        Listening {
            signals: self.clone(),
            lobby: lobby.to_owned(),
            rx,
        }
    }
}

struct Listening {
    signals: Signals,
    lobby: String,
    rx: mpsc::UnboundedReceiver<(String, Value)>,
}

impl Listening {
    async fn recv(&mut self) -> Option<(String, Signal)> {
        loop {
            let (from, data) = self.rx.recv().await?;
            match serde_json::from_value(data) {
                Ok(signal) => return Some((from, signal)),
                Err(e) => tracing::debug!("play: unknown signal from {from}: {e}"),
            }
        }
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.signals.0.lock().expect("signals").remove(&self.lobby);
    }
}

/// Without STUN and TURN only players on one network reach each other, which still beats nothing.
async fn ice() -> crate::net::IceServers {
    lobby::ice().await.unwrap_or_else(|e| {
        tracing::warn!("play: no STUN or TURN servers: {e}");
        Default::default()
    })
}

fn connect_error(e: std::io::Error) -> PlayError {
    PlayError::Connect(e.to_string())
}

fn to_value(signal: &Signal) -> Value {
    serde_json::to_value(signal).unwrap_or_default()
}

struct Tasks(Mutex<Vec<JoinHandle<()>>>);

impl Tasks {
    fn new(tasks: Vec<JoinHandle<()>>) -> Self {
        Self(Mutex::new(tasks))
    }

    fn stop(&self) {
        for task in self.0.lock().expect("tasks").drain(..) {
            task.abort();
        }
    }
}

impl Drop for Tasks {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// A guest's connection came up; carries their BX ID id.
    Joined(String),
    Left(String),
}

/// A world open to friends: guests' game connections reach the LAN port of this game.
pub struct Host {
    pub lobby: Lobby,
    port: Arc<AtomicU16>,
    peer: Arc<Peer>,
    guests: Arc<Mutex<HashMap<Fingerprint, String>>>,
    /// Each guest's live connections, to cut them when the guest is sent out.
    connections: Arc<Mutex<HashMap<String, Vec<quinn::Connection>>>>,
    tasks: Tasks,
}

impl Host {
    pub async fn open(
        name: &str,
        game: &LobbyGame,
        open: bool,
        port: u16,
        signaller: Signaller,
        signals: &Signals,
        on_event: impl Fn(HostEvent) + Send + Sync + 'static,
    ) -> Result<Self, PlayError> {
        let peer = Arc::new(
            Peer::bind(&ice().await, true)
                .await
                .map_err(connect_error)?,
        );
        let lobby = lobby::create(name, game, open).await?;
        let mut inbox = signals.listen(&lobby.id);
        let guests: Arc<Mutex<HashMap<Fingerprint, String>>> = Arc::default();
        let port = Arc::new(AtomicU16::new(port));

        let answering = tokio::spawn({
            let peer = peer.clone();
            let guests = guests.clone();
            let lobby = lobby.id.clone();
            async move {
                while let Some((from, signal)) = inbox.recv().await {
                    let Signal::Offer(offer) = signal else {
                        continue;
                    };
                    match peer.expect(&offer) {
                        Ok(fingerprint) => {
                            guests
                                .lock()
                                .expect("guests")
                                .insert(fingerprint, from.clone());
                            signaller.send(&lobby, &from, to_value(&Signal::Answer(peer.offer())));
                        }
                        Err(e) => tracing::debug!("play: bad offer from {from}: {e}"),
                    }
                }
            }
        });

        let connections: Arc<Mutex<HashMap<String, Vec<quinn::Connection>>>> = Arc::default();
        let accepting = tokio::spawn({
            let peer = peer.clone();
            let port = port.clone();
            let guests = guests.clone();
            let connections = connections.clone();
            let on_event = Arc::new(on_event);
            async move {
                // A guest tries several paths at once, so one guest can briefly hold more than one.
                let open: Arc<Mutex<HashMap<String, usize>>> = Arc::default();
                while let Some((connection, fingerprint)) = peer.accept().await {
                    let Some(user) = guests.lock().expect("guests").get(&fingerprint).cloned()
                    else {
                        connection.close(1u32.into(), b"unknown");
                        continue;
                    };
                    let first = {
                        let mut open = open.lock().expect("open");
                        let n = open.entry(user.clone()).or_default();
                        *n += 1;
                        *n == 1
                    };
                    if first {
                        on_event(HostEvent::Joined(user.clone()));
                    }
                    connections
                        .lock()
                        .expect("connections")
                        .entry(user.clone())
                        .or_default()
                        .push(connection.clone());
                    let (port, open, on_event) = (port.clone(), open.clone(), on_event.clone());
                    let connections = connections.clone();
                    tokio::spawn(async move {
                        serve(&connection, &port).await;
                        if let Some(list) = connections.lock().expect("connections").get_mut(&user)
                        {
                            list.retain(|c| c.stable_id() != connection.stable_id());
                        }
                        let last = {
                            let mut open = open.lock().expect("open");
                            let n = open.entry(user.clone()).or_default();
                            *n = n.saturating_sub(1);
                            *n == 0
                        };
                        if last {
                            on_event(HostEvent::Left(user));
                        }
                    });
                }
            }
        });

        Ok(Self {
            lobby,
            port,
            peer,
            guests,
            connections,
            tasks: Tasks::new(vec![answering, accepting]),
        })
    }

    /// The world was opened to LAN again on another port.
    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    pub async fn invite(&self, user_id: &str) -> Result<Lobby, PlayError> {
        lobby::invite(&self.lobby.id, user_id).await
    }

    /// Sends a guest out of the lobby and cuts their connection; also takes an invite back.
    pub async fn kick(&self, user_id: &str) -> Result<Lobby, PlayError> {
        let lobby = lobby::kick(&self.lobby.id, user_id).await?;
        self.guests
            .lock()
            .expect("guests")
            .retain(|_, user| user != user_id);
        let cut = self
            .connections
            .lock()
            .expect("connections")
            .remove(user_id)
            .unwrap_or_default();
        for connection in cut {
            connection.close(KICKED.into(), b"kicked");
        }
        Ok(lobby)
    }

    /// Closes the lobby and every guest's connection.
    pub async fn close(&self) -> Result<(), PlayError> {
        self.tasks.stop();
        self.peer.close().await;
        lobby::close(&self.lobby.id).await
    }
}

/// Passes each stream a guest opens to the game's LAN port until the connection ends.
async fn serve(connection: &quinn::Connection, port: &AtomicU16) {
    while let Ok((send, mut recv)) = connection.accept_bi().await {
        let port = port.load(Ordering::Relaxed);
        tokio::spawn(async move {
            if recv.read_u8().await.ok() != Some(GAME_STREAM) {
                return;
            }
            match TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await {
                Ok(tcp) => pipe(tcp, send, recv).await,
                Err(e) => tracing::debug!("play: the world on port {port} is gone: {e}"),
            }
        });
    }
}

async fn pipe(tcp: TcpStream, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
    let _ = tcp.set_nodelay(true);
    let (mut read, mut write) = tcp.into_split();
    let up = async {
        let _ = tokio::io::copy(&mut read, &mut send).await;
        let _ = send.finish();
    };
    let down = async {
        let _ = tokio::io::copy(&mut recv, &mut write).await;
        let _ = write.shutdown().await;
    };
    tokio::join!(up, down);
}

/// A joined lobby: the host's world is reachable at `127.0.0.1:port` and shows under LAN.
pub struct Guest {
    pub lobby: Lobby,
    pub port: u16,
    peer: Arc<Peer>,
    tasks: Tasks,
}

impl Guest {
    /// Joins `lobby_id` and connects to its host; `on_lost` runs once if the connection drops,
    /// told whether the host sent this player out.
    pub async fn join(
        lobby_id: &str,
        signaller: Signaller,
        signals: &Signals,
        on_lost: impl FnOnce(bool) + Send + 'static,
    ) -> Result<Self, PlayError> {
        let lobby = lobby::join(lobby_id).await?;
        let peer = Arc::new(
            Peer::bind(&ice().await, false)
                .await
                .map_err(connect_error)?,
        );
        let mut inbox = signals.listen(&lobby.id);
        let offer = to_value(&Signal::Offer(peer.offer()));
        let answer = tokio::time::timeout(ANSWER_TIMEOUT, async {
            loop {
                signaller.send(&lobby.id, &lobby.host, offer.clone());
                let deadline = tokio::time::sleep(OFFER_AGAIN);
                tokio::pin!(deadline);
                loop {
                    tokio::select! {
                        got = inbox.recv() => match got {
                            Some((from, Signal::Answer(answer))) if from == lobby.host => return Some(answer),
                            Some(_) => {}
                            None => return None,
                        },
                        _ = &mut deadline => break,
                    }
                }
            }
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| PlayError::Connect("the host did not answer".into()))?;
        drop(inbox);
        let connection = peer.dial(&answer).await.map_err(connect_error)?;

        let listener = TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))
            .await
            .map_err(connect_error)?;
        let port = listener.local_addr().map_err(connect_error)?.port();
        let forwarding = tokio::spawn({
            let connection = connection.clone();
            async move {
                while let Ok((tcp, from)) = listener.accept().await {
                    if !from_this_machine(from) {
                        continue;
                    }
                    let connection = connection.clone();
                    tokio::spawn(async move {
                        let Ok((mut send, recv)) = connection.open_bi().await else {
                            return;
                        };
                        if send.write_u8(GAME_STREAM).await.is_ok() {
                            pipe(tcp, send, recv).await;
                        }
                    });
                }
            }
        });
        let announcing = lan::announce(lobby.name.clone(), port);
        let watching = tokio::spawn(async move {
            let reason = connection.closed().await;
            tracing::debug!("play: connection to the host closed: {reason}");
            let kicked = matches!(
                &reason,
                quinn::ConnectionError::ApplicationClosed(close) if close.error_code == KICKED.into()
            );
            on_lost(kicked);
        });
        Ok(Self {
            lobby,
            port,
            peer,
            tasks: Tasks::new(vec![forwarding, announcing, watching]),
        })
    }

    pub async fn leave(&self) -> Result<(), PlayError> {
        // Stopped first, or closing on purpose would report a lost connection.
        self.tasks.stop();
        self.peer.close().await;
        lobby::leave(&self.lobby.id).await
    }
}

/// The listener is on every interface because the game connects to the address a LAN
/// announcement came from; only this machine gets in.
fn from_this_machine(from: SocketAddr) -> bool {
    match from.ip() {
        std::net::IpAddr::V4(ip) => ip.is_loopback() || crate::net::local_ips().contains(&ip),
        std::net::IpAddr::V6(ip) => ip.is_loopback(),
    }
}
