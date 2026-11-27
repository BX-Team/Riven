pub mod socket;
pub mod stun;
pub mod tls;
pub mod turn;

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt as _;
use futures_util::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};

use socket::{Paths, Route, VirtualSocket};
use stun::Message;
use tls::{Allowed, Fingerprint, Identity};
use turn::{Allocation, Server, Transport};

const STUN_TIMEOUT: Duration = Duration::from_secs(2);
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);
const PUNCH_EVERY: Duration = Duration::from_millis(200);
const PUNCHES: u32 = 40;
/// What a host sends first on a connection it let in.
const WELCOME: u8 = 0x52;

/// STUN and TURN servers with credentials, as `/v1/play/ice` hands them out.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IceServers {
    pub ice_servers: Vec<IceServer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IceServer {
    pub urls: Urls,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub credential: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Urls {
    One(String),
    Many(Vec<String>),
}

impl IceServers {
    fn servers(&self) -> (Vec<Server>, Vec<Server>) {
        let (mut stun, mut turn) = (Vec::new(), Vec::new());
        for server in &self.ice_servers {
            let urls = match &server.urls {
                Urls::One(url) => std::slice::from_ref(url),
                Urls::Many(urls) => urls.as_slice(),
            };
            for url in urls {
                match Server::parse(url, &server.username, &server.credential) {
                    Some((false, s)) => stun.push(s),
                    Some((true, s)) => turn.push(s),
                    None => {}
                }
            }
        }
        (stun, turn)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An address of this machine, reachable on the same network.
    Host,
    /// The address a NAT maps the socket to, as a STUN server sees it.
    Srflx,
    /// An address on a TURN server that forwards to this side.
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub kind: Kind,
    pub addr: SocketAddr,
}

/// What one side tells the other through the lobby to be reached and recognised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// SHA-256 of the side's certificate, in hex.
    pub cert: String,
    pub candidates: Vec<Candidate>,
}

impl Offer {
    fn fingerprint(&self) -> io::Result<Fingerprint> {
        hex::decode(&self.cert)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "bad certificate fingerprint")
            })
    }
}

/// Whether to skip direct paths; `RIVEN_PLAY_RELAY=1` forces every connection through TURN.
fn relay_only() -> bool {
    std::env::var("RIVEN_PLAY_RELAY").is_ok_and(|v| v == "1")
}

type Waiters = Arc<Mutex<HashMap<[u8; 12], oneshot::Sender<Message>>>>;

/// One side of a game connection: a QUIC endpoint reachable at its candidates.
pub struct Peer {
    endpoint: quinn::Endpoint,
    socket: Arc<VirtualSocket>,
    identity: Identity,
    allowed: Allowed,
    candidates: Vec<Candidate>,
    accepted: tokio::sync::Mutex<mpsc::UnboundedReceiver<(quinn::Connection, Fingerprint)>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Peer {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        self.endpoint.close(0u32.into(), b"");
    }
}

impl Peer {
    /// Opens the socket, learns its public address and takes relays; `serve` lets peers connect in.
    pub async fn bind(ice: &IceServers, serve: bool) -> io::Result<Self> {
        let direct = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
        let paths = Arc::new(Paths::default());
        let (socket, inbox) = VirtualSocket::new(direct.clone(), paths.clone());
        let socket = Arc::new(socket);
        let waiters: Waiters = Arc::default();

        let reader = tokio::spawn({
            let waiters = waiters.clone();
            let inbox = inbox.clone();
            let paths = paths.clone();
            async move {
                let mut buf = vec![0; 2048];
                loop {
                    let Ok((n, from)) = direct.recv_from(&mut buf).await else {
                        // Windows reports an ICMP port unreachable as an error on the next read.
                        tokio::task::yield_now().await;
                        continue;
                    };
                    let data = &buf[..n];
                    if stun::is_stun(data) {
                        if let Some(message) = Message::decode(data)
                            && let Some(waiter) =
                                waiters.lock().expect("waiters").remove(&message.tid)
                        {
                            let _ = waiter.send(message);
                        }
                        continue;
                    }
                    let addr = paths.virtual_addr(Route::Direct, from);
                    if inbox.send((addr, Bytes::copy_from_slice(data))).is_err() {
                        return;
                    }
                }
            }
        });

        let (stun_servers, turn_servers) = ice.servers();
        let port = socket.direct.local_addr()?.port();
        let mut candidates: Vec<Candidate> = local_ips()
            .into_iter()
            .filter(|ip| !ip.is_loopback() && !ip.is_link_local())
            .map(|ip| Candidate {
                kind: Kind::Host,
                addr: SocketAddr::new(ip.into(), port),
            })
            .collect();
        let relays = pick_relays(&turn_servers);
        let allocations = relays.iter().enumerate().map(|(i, server)| {
            let inbox = inbox.clone();
            let paths = paths.clone();
            let inbound: turn::Inbound = Arc::new(move |peer, data| {
                let _ = inbox.send((paths.virtual_addr(Route::Relay(i), peer), data));
            });
            async move {
                match turn::allocate(server, inbound).await {
                    Ok(allocation) => Some(allocation),
                    Err(e) => {
                        tracing::debug!(
                            "play: no {:?} relay from {}: {e}",
                            server.transport,
                            server.host
                        );
                        None
                    }
                }
            }
        });
        let (srflx, allocations) = tokio::join!(
            reflexive(&socket.direct, &stun_servers, &waiters),
            futures_util::future::join_all(allocations)
        );
        if let Some(addr) = srflx
            && !candidates.iter().any(|c| c.addr == addr)
        {
            candidates.push(Candidate {
                kind: Kind::Srflx,
                addr,
            });
        }
        candidates.extend(allocations.iter().flatten().map(|a| Candidate {
            kind: Kind::Relay,
            addr: a.relayed,
        }));
        if relay_only() {
            candidates.retain(|c| c.kind == Kind::Relay);
        }
        let _ = socket.relays.set(allocations);
        if candidates.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "no network path",
            ));
        }
        tracing::debug!("play: candidates {candidates:?}");

        let identity = Identity::generate().map_err(io::Error::other)?;
        let allowed = Allowed::default();
        let server = serve
            .then(|| tls::server_config(&identity, allowed.clone()))
            .transpose()
            .map_err(io::Error::other)?;
        let endpoint = quinn::Endpoint::new_with_abstract_socket(
            quinn::EndpointConfig::default(),
            server,
            socket.clone(),
            Arc::new(quinn::TokioRuntime),
        )?;
        let (accepted_tx, accepted) = mpsc::unbounded_channel();
        // Handshakes run side by side: a path that only works one way would hold up the rest.
        let acceptor = tokio::spawn({
            let endpoint = endpoint.clone();
            async move {
                while let Some(incoming) = endpoint.accept().await {
                    let accepted = accepted_tx.clone();
                    tokio::spawn(async move {
                        if let Some(peer) = admit(incoming).await {
                            let _ = accepted.send(peer);
                        }
                    });
                }
            }
        });
        Ok(Self {
            endpoint,
            socket,
            identity,
            allowed,
            candidates,
            accepted: tokio::sync::Mutex::new(accepted),
            tasks: vec![reader, acceptor],
        })
    }

    pub fn offer(&self) -> Offer {
        Offer {
            cert: hex::encode(self.identity.fingerprint),
            candidates: self.candidates.clone(),
        }
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.identity.fingerprint
    }

    fn relays(&self) -> impl Iterator<Item = (usize, &Allocation)> {
        self.socket
            .relays
            .get()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, a)| a.as_ref().map(|a| (i, a)))
    }

    /// Lets the peer of `remote` in and opens this side's NAT and relays towards it.
    pub fn expect(&self, remote: &Offer) -> io::Result<Fingerprint> {
        let fingerprint = remote.fingerprint()?;
        self.allowed.lock().expect("allowed").insert(fingerprint);
        let ips: Vec<IpAddr> = remote.candidates.iter().map(|c| c.addr.ip()).collect();
        for (_, relay) in self.relays() {
            relay.permit(ips.clone());
        }
        let routes = self.routes(remote);
        let socket = self.socket.clone();
        tokio::spawn(async move {
            let punch = Message::new(stun::BINDING_INDICATION).encode(None);
            for _ in 0..PUNCHES {
                for (_, route, to) in &routes {
                    socket.send_raw(*route, *to, &punch);
                }
                tokio::time::sleep(PUNCH_EVERY).await;
            }
        });
        Ok(fingerprint)
    }

    /// Every way to reach `remote`, with how long to wait before trying it: direct ones first.
    fn routes(&self, remote: &Offer) -> Vec<(Duration, Route, SocketAddr)> {
        let mut routes = Vec::new();
        if !relay_only() {
            for c in &remote.candidates {
                let delay = match c.kind {
                    Kind::Host | Kind::Srflx => Duration::ZERO,
                    Kind::Relay => Duration::from_millis(500),
                };
                routes.push((delay, Route::Direct, c.addr));
            }
        }
        for (i, relay) in self.relays() {
            let delay = match relay.transport {
                Transport::Udp => Duration::from_secs(1),
                Transport::Tcp | Transport::Tls => Duration::from_secs(2),
            };
            for c in remote.candidates.iter().filter(|c| c.kind != Kind::Host) {
                routes.push((delay, Route::Relay(i), c.addr));
            }
        }
        routes
    }

    /// Connects to the peer of `remote` over whichever path answers first.
    pub async fn dial(&self, remote: &Offer) -> io::Result<quinn::Connection> {
        let fingerprint = self.expect(remote)?;
        let config = tls::client_config(&self.identity, fingerprint).map_err(io::Error::other)?;
        let mut attempts: FuturesUnordered<_> = self
            .routes(remote)
            .into_iter()
            .map(|(delay, route, to)| {
                let addr = self.socket.paths.virtual_addr(route, to);
                let connecting = self
                    .endpoint
                    .connect_with(config.clone(), addr, tls::SERVER_NAME);
                async move {
                    tokio::time::sleep(delay).await;
                    let connection = welcomed(connecting.map_err(io::Error::other)?).await;
                    tracing::debug!("play: {route:?} to {to}: {:?}", connection.as_ref().err());
                    connection
                }
            })
            .collect();
        let first = async {
            let mut last = io::Error::new(io::ErrorKind::NotConnected, "no path to the host");
            while let Some(attempt) = attempts.next().await {
                match attempt {
                    Ok(connection) => return Ok(connection),
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => return Err(e),
                    Err(e) => last = e,
                }
            }
            Err(last)
        };
        tokio::time::timeout(DIAL_TIMEOUT, first)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the host did not answer"))?
    }

    /// The next peer that connected and showed an expected certificate.
    pub async fn accept(&self) -> Option<(quinn::Connection, Fingerprint)> {
        self.accepted.lock().await.recv().await
    }

    /// Tells every connected peer this side is gone, waiting a moment for it to hear.
    pub async fn close(&self) {
        self.endpoint.close(0u32.into(), b"closed");
        let _ = tokio::time::timeout(Duration::from_secs(1), self.endpoint.wait_idle()).await;
    }

    /// Stops letting a peer in; its open connections stay until closed.
    pub fn forget(&self, fingerprint: &Fingerprint) {
        self.allowed.lock().expect("allowed").remove(fingerprint);
    }
}

async fn admit(incoming: quinn::Incoming) -> Option<(quinn::Connection, Fingerprint)> {
    let connection = incoming.await.ok()?;
    let fingerprint = tls::peer_fingerprint(&connection)?;
    let mut welcome = connection.open_uni().await.ok()?;
    welcome.write_all(&[WELCOME]).await.ok()?;
    welcome.finish().ok()?;
    Some((connection, fingerprint))
}

/// A client's handshake ends before the host checks its certificate, so a path counts
/// only once the host says it let this side in.
async fn welcomed(connecting: quinn::Connecting) -> io::Result<quinn::Connection> {
    let connection = connecting.await.map_err(lost)?;
    let mut welcome = connection.accept_uni().await.map_err(lost)?;
    let mut byte = [0];
    welcome.read_exact(&mut byte).await.map_err(|e| match e {
        quinn::ReadExactError::ReadError(quinn::ReadError::ConnectionLost(e)) => lost(e),
        e => io::Error::other(e),
    })?;
    if byte[0] != WELCOME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a Riven host",
        ));
    }
    Ok(connection)
}

/// A close the host sent itself is its refusal; timeouts and resets only say this path is bad.
fn lost(e: quinn::ConnectionError) -> io::Error {
    let kind = match e {
        quinn::ConnectionError::ConnectionClosed(_)
        | quinn::ConnectionError::ApplicationClosed(_) => io::ErrorKind::ConnectionRefused,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, e)
}

/// One UDP relay and one over TLS: the second gets through networks that only allow HTTPS.
fn pick_relays(servers: &[Server]) -> Vec<Server> {
    let udp = servers
        .iter()
        .filter(|s| s.transport == Transport::Udp)
        .min_by_key(|s| s.port != 3478);
    let tls = servers
        .iter()
        .filter(|s| s.transport == Transport::Tls)
        .min_by_key(|s| s.port != 443);
    udp.into_iter().chain(tls).cloned().collect()
}

/// The address the first answering STUN server sees `socket` at.
async fn reflexive(
    socket: &UdpSocket,
    servers: &[Server],
    waiters: &Waiters,
) -> Option<SocketAddr> {
    for server in servers.iter().filter(|s| s.transport == Transport::Udp) {
        let Ok(addr) = server.resolve().await else {
            continue;
        };
        if !addr.is_ipv4() {
            continue;
        }
        let request = Message::new(stun::BINDING_REQUEST);
        let (tx, rx) = oneshot::channel();
        waiters.lock().expect("waiters").insert(request.tid, tx);
        let raw = request.encode(None);
        let answer = async {
            for _ in 0..3 {
                let _ = socket.send_to(&raw, addr).await;
                tokio::time::sleep(STUN_TIMEOUT / 3).await;
            }
        };
        let answer = tokio::select! {
            got = rx => got.ok(),
            _ = tokio::time::sleep(STUN_TIMEOUT) => None,
            _ = answer => None,
        };
        waiters.lock().expect("waiters").remove(&request.tid);
        if let Some(addr) = answer.and_then(|m| m.addr(stun::XOR_MAPPED_ADDRESS)) {
            return Some(addr);
        }
    }
    None
}

/// The IPv4 addresses of this machine's interfaces, loopback included.
pub(crate) fn local_ips() -> Vec<std::net::Ipv4Addr> {
    let interfaces = if_addrs::get_if_addrs().unwrap_or_default();
    let mut ips: Vec<_> = interfaces
        .iter()
        .filter(|i| i.oper_status != if_addrs::IfOperStatus::Down)
        .filter_map(|i| match i.ip() {
            IpAddr::V4(ip) => Some(ip),
            IpAddr::V6(_) => None,
        })
        .collect();
    ips.sort();
    ips.dedup();
    ips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn peers_connect_over_host_candidates_and_refuse_strangers() {
        let host = Peer::bind(&IceServers::default(), true).await.unwrap();
        let guest = Peer::bind(&IceServers::default(), false).await.unwrap();
        let stranger = Peer::bind(&IceServers::default(), false).await.unwrap();
        let mut host_offer = host.offer();
        host_offer.candidates.push(Candidate {
            kind: Kind::Host,
            addr: "192.0.2.1:9".parse().unwrap(),
        });
        let expected = host.expect(&guest.offer()).unwrap();
        assert_eq!(expected, guest.fingerprint());

        let (accepted, mut accepted_rx) = tokio::sync::mpsc::unbounded_channel();
        let _host = tokio::spawn(async move {
            // A guest tries every path at once and drops the losers, so each one is served apart.
            while let Some((connection, fingerprint)) = host.accept().await {
                let _ = accepted.send(fingerprint);
                tokio::spawn(async move {
                    let Ok((mut send, mut recv)) = connection.accept_bi().await else {
                        return;
                    };
                    let got = recv.read_to_end(64).await.unwrap();
                    send.write_all(&got).await.unwrap();
                    send.finish().unwrap();
                    connection.closed().await;
                });
            }
        });

        let connection = guest.dial(&host_offer).await.unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        send.write_all(b"hello").await.unwrap();
        send.finish().unwrap();
        assert_eq!(recv.read_to_end(64).await.unwrap(), b"hello");
        while let Ok(fingerprint) = accepted_rx.try_recv() {
            assert_eq!(fingerprint, guest.fingerprint());
        }

        // The host never saw this one's offer, so the handshake fails on its certificate.
        let refused =
            tokio::time::timeout(Duration::from_secs(5), stranger.dial(&host_offer)).await;
        assert!(matches!(refused, Ok(Err(_))), "{refused:?}");
        assert!(accepted_rx.try_recv().is_err());
    }
}
