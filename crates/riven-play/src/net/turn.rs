use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::io::{
    AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, ReadHalf, WriteHalf,
};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;

use super::stun::{self, Message};

const LIFETIME_SECS: u32 = 600;
/// Permissions last five minutes and allocations ten; both are renewed well before.
const RENEW: Duration = Duration::from_secs(240);
const RETRANSMIT: Duration = Duration::from_millis(500);
const MAX_TRIES: u32 = 6;
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);
const UDP: u8 = 17;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Udp,
    Tcp,
    Tls,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub transport: Transport,
    pub host: String,
    pub port: u16,
    /// Empty for a STUN server.
    pub username: String,
    pub credential: String,
}

impl Server {
    /// Reads a `stun:`, `turn:` or `turns:` URL; `None` for anything else.
    pub fn parse(url: &str, username: &str, credential: &str) -> Option<(bool, Self)> {
        let (scheme, rest) = url.split_once(':')?;
        let (address, query) = rest.split_once('?').unwrap_or((rest, ""));
        let transport = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("transport="))
            .unwrap_or("udp");
        let (relay, transport, default_port) = match (scheme, transport) {
            ("stun", _) => (false, Transport::Udp, 3478),
            ("turn", "udp") => (true, Transport::Udp, 3478),
            ("turn", "tcp") => (true, Transport::Tcp, 3478),
            ("turns", "tcp") => (true, Transport::Tls, 5349),
            _ => return None,
        };
        let (host, port) = match address.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || address.starts_with('[') => {
                (host, port.parse().ok()?)
            }
            _ => (address, default_port),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        Some((
            relay,
            Self {
                transport,
                host: host.to_owned(),
                port,
                username: username.to_owned(),
                credential: credential.to_owned(),
            },
        ))
    }

    pub async fn resolve(&self) -> io::Result<SocketAddr> {
        let mut all: Vec<SocketAddr> = tokio::net::lookup_host((self.host.as_str(), self.port))
            .await?
            .collect();
        all.sort_by_key(|a| !a.is_ipv4());
        all.into_iter()
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))
    }
}

/// What reaches this side through the relay: the peer's address and its datagram.
pub type Inbound = Arc<dyn Fn(SocketAddr, Bytes) + Send + Sync>;

enum Command {
    Send(SocketAddr, Vec<u8>),
    Permit(Vec<IpAddr>),
}

/// A relayed address on a TURN server; dropping it releases the allocation.
pub struct Allocation {
    pub relayed: SocketAddr,
    pub transport: Transport,
    commands: mpsc::UnboundedSender<Command>,
}

impl Allocation {
    pub fn send(&self, peer: SocketAddr, data: &[u8]) {
        let _ = self.commands.send(Command::Send(peer, data.to_vec()));
    }

    /// Lets these addresses send to the relayed address; TURN drops everyone else.
    pub fn permit(&self, ips: Vec<IpAddr>) {
        let _ = self.commands.send(Command::Permit(ips));
    }
}

impl std::fmt::Debug for Allocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Allocation")
            .field("relayed", &self.relayed)
            .field("transport", &self.transport)
            .finish()
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

enum Writer {
    Udp(Arc<UdpSocket>),
    Stream(WriteHalf<Box<dyn Io>>),
}

impl Writer {
    async fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Udp(socket) => socket.send(buf).await.map(|_| ()),
            Self::Stream(stream) => stream.write_all(buf).await,
        }
    }
}

#[derive(Clone)]
enum Request {
    Refresh(u32),
    Permit(Vec<IpAddr>),
}

impl Request {
    fn message(&self, tid: Option<[u8; 12]>) -> Message {
        let mut message = Message::new(match self {
            Self::Refresh(_) => stun::REFRESH,
            Self::Permit(_) => stun::CREATE_PERMISSION,
        });
        if let Some(tid) = tid {
            message.tid = tid;
        }
        match self {
            Self::Refresh(secs) => message.with(stun::LIFETIME, secs.to_be_bytes()),
            Self::Permit(ips) => ips.iter().fold(message, |m, ip| {
                m.with_addr(stun::XOR_PEER_ADDRESS, SocketAddr::new(*ip, 0))
            }),
        }
    }
}

struct Pending {
    request: Request,
    sent: Instant,
    tries: u32,
}

struct Client {
    server: Server,
    writer: Writer,
    frames: mpsc::UnboundedReceiver<Vec<u8>>,
    realm: Vec<u8>,
    nonce: Vec<u8>,
    key: Option<Vec<u8>>,
    reliable: bool,
}

impl Client {
    async fn send(&mut self, message: Message) -> io::Result<()> {
        let message = if self.key.is_some() {
            message
                .with(stun::USERNAME, self.server.username.as_bytes())
                .with(stun::REALM, self.realm.clone())
                .with(stun::NONCE, self.nonce.clone())
        } else {
            message
        };
        let raw = message.encode(self.key.as_deref());
        self.writer.write(&raw).await
    }

    /// One request during setup, retransmitted over UDP and answered by its transaction id.
    async fn request(&mut self, message: Message) -> io::Result<Message> {
        let deadline = tokio::time::Instant::now() + SETUP_TIMEOUT;
        let mut wait = RETRANSMIT;
        loop {
            self.send(message.clone()).await?;
            let resend = tokio::time::Instant::now() + wait;
            loop {
                let until = if self.reliable {
                    deadline
                } else {
                    resend.min(deadline)
                };
                match tokio::time::timeout_at(until, self.frames.recv()).await {
                    Ok(Some(raw)) => {
                        if let Some(answer) = Message::decode(&raw)
                            && answer.tid == message.tid
                        {
                            return Ok(answer);
                        }
                    }
                    Ok(None) => return Err(io::ErrorKind::ConnectionAborted.into()),
                    Err(_) if tokio::time::Instant::now() >= deadline => {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    Err(_) => break,
                }
            }
            wait *= 2;
        }
    }

    /// Sends a request whose answer the run loop picks up.
    async fn post(&mut self, pending: &mut HashMap<[u8; 12], Pending>, request: Request) {
        let message = request.message(None);
        let tid = message.tid;
        if self.send(message).await.is_ok() {
            pending.insert(
                tid,
                Pending {
                    request,
                    sent: Instant::now(),
                    tries: 1,
                },
            );
        }
    }

    /// Picks up the realm and nonce of a 401 or 438 answer; `false` when it was neither.
    fn challenged(&mut self, answer: &Message) -> bool {
        if !matches!(answer.error_code(), Some(401 | 438)) {
            return false;
        }
        let (Some(realm), Some(nonce)) = (answer.attr(stun::REALM), answer.attr(stun::NONCE))
        else {
            return false;
        };
        self.realm = realm.to_vec();
        self.nonce = nonce.to_vec();
        self.key = Some(stun::long_term_key(
            &self.server.username,
            &String::from_utf8_lossy(realm),
            &self.server.credential,
        ));
        true
    }
}

/// Allocates a relayed UDP address; `inbound` gets what peers send to it.
pub async fn allocate(server: &Server, inbound: Inbound) -> io::Result<Allocation> {
    let addr = server.resolve().await?;
    let (frames_tx, frames) = mpsc::unbounded_channel();
    let writer = match server.transport {
        Transport::Udp => {
            let socket = UdpSocket::bind(if addr.is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            })
            .await?;
            socket.connect(addr).await?;
            let socket = Arc::new(socket);
            let reader = socket.clone();
            tokio::spawn(async move {
                let mut buf = vec![0; 2048];
                while let Ok(n) = reader.recv(&mut buf).await {
                    if frames_tx.send(buf[..n].to_vec()).is_err() {
                        return;
                    }
                }
            });
            Writer::Udp(socket)
        }
        Transport::Tcp | Transport::Tls => {
            let tcp = tokio::time::timeout(SETUP_TIMEOUT, TcpStream::connect(addr))
                .await
                .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
            tcp.set_nodelay(true)?;
            let stream: Box<dyn Io> = if server.transport == Transport::Tls {
                let name = rustls::pki_types::ServerName::try_from(server.host.clone())
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
                let tls = tokio::time::timeout(SETUP_TIMEOUT, tls()?.connect(name, tcp))
                    .await
                    .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
                Box::new(tls)
            } else {
                Box::new(tcp)
            };
            let (read, write) = tokio::io::split(stream);
            tokio::spawn(read_frames(read, frames_tx));
            Writer::Stream(write)
        }
    };
    let mut client = Client {
        server: server.clone(),
        writer,
        frames,
        realm: Vec::new(),
        nonce: Vec::new(),
        key: None,
        reliable: server.transport != Transport::Udp,
    };

    let allocate = || {
        Message::new(stun::ALLOCATE)
            .with(stun::REQUESTED_TRANSPORT, [UDP, 0, 0, 0])
            .with(stun::LIFETIME, LIFETIME_SECS.to_be_bytes())
    };
    let mut answer = client.request(allocate()).await?;
    for _ in 0..2 {
        if !client.challenged(&answer) {
            break;
        }
        answer = client.request(allocate()).await?;
    }
    if !stun::is_success(answer.kind) {
        let code = answer.error_code().unwrap_or(0);
        return Err(io::Error::other(format!(
            "TURN refused the allocation ({code})"
        )));
    }
    let relayed = answer
        .addr(stun::XOR_RELAYED_ADDRESS)
        .ok_or_else(|| io::Error::other("TURN gave no relayed address"))?;

    let (commands, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(client, rx, inbound));
    Ok(Allocation {
        relayed,
        transport: server.transport,
        commands,
    })
}

async fn run(mut client: Client, mut commands: mpsc::UnboundedReceiver<Command>, inbound: Inbound) {
    let mut permitted: HashSet<IpAddr> = HashSet::new();
    let mut pending: HashMap<[u8; 12], Pending> = HashMap::new();
    let mut renew = tokio::time::interval(RENEW);
    renew.tick().await;
    let mut retransmit = tokio::time::interval(RETRANSMIT);

    loop {
        tokio::select! {
            frame = client.frames.recv() => {
                let Some(raw) = frame else { return };
                let Some(message) = Message::decode(&raw) else { continue };
                if message.kind == stun::DATA_INDICATION {
                    if let (Some(peer), Some(data)) = (message.addr(stun::XOR_PEER_ADDRESS), message.attr(stun::DATA)) {
                        inbound(peer, Bytes::copy_from_slice(data));
                    }
                    continue;
                }
                let Some(done) = pending.remove(&message.tid) else { continue };
                if stun::is_error(message.kind) {
                    if client.challenged(&message) {
                        client.post(&mut pending, done.request).await;
                    } else {
                        tracing::debug!("turn: request refused ({:?})", message.error_code());
                    }
                }
            }
            command = commands.recv() => match command {
                None => {
                    let _ = client.send(Request::Refresh(0).message(None)).await;
                    return;
                }
                Some(Command::Send(peer, data)) => {
                    let message = Message::new(stun::SEND_INDICATION)
                        .with_addr(stun::XOR_PEER_ADDRESS, peer)
                        .with(stun::DATA, data);
                    if client.writer.write(&message.encode(None)).await.is_err() {
                        return;
                    }
                }
                Some(Command::Permit(ips)) => {
                    let new: Vec<IpAddr> = ips.into_iter().filter(|ip| permitted.insert(*ip)).collect();
                    if !new.is_empty() {
                        client.post(&mut pending, Request::Permit(new)).await;
                    }
                }
            },
            _ = renew.tick() => {
                client.post(&mut pending, Request::Refresh(LIFETIME_SECS)).await;
                if !permitted.is_empty() {
                    client.post(&mut pending, Request::Permit(permitted.iter().copied().collect())).await;
                }
            }
            _ = retransmit.tick(), if !client.reliable && !pending.is_empty() => {
                let due: Vec<[u8; 12]> = pending
                    .iter()
                    .filter(|(_, p)| p.sent.elapsed() >= RETRANSMIT * p.tries)
                    .map(|(tid, _)| *tid)
                    .collect();
                for tid in due {
                    let Some(mut waiting) = pending.remove(&tid) else { continue };
                    if waiting.tries >= MAX_TRIES {
                        tracing::debug!("turn: request unanswered");
                        continue;
                    }
                    let _ = client.send(waiting.request.message(Some(tid))).await;
                    waiting.tries += 1;
                    pending.insert(tid, waiting);
                }
            }
        }
    }
}

/// Splits a TURN byte stream into STUN messages; channel data frames are skipped.
async fn read_frames(mut read: ReadHalf<Box<dyn Io>>, frames: mpsc::UnboundedSender<Vec<u8>>) {
    loop {
        let mut head = [0u8; 4];
        if read.read_exact(&mut head).await.is_err() {
            return;
        }
        let (len, stun) = if head[0] & 0xc0 == 0 {
            (stun::framed_len(&head), true)
        } else {
            // Over TCP, channel data is padded to four bytes.
            (
                4 + (u16::from_be_bytes([head[2], head[3]]) as usize).next_multiple_of(4),
                false,
            )
        };
        let mut frame = head.to_vec();
        frame.resize(len, 0);
        if read.read_exact(&mut frame[4..]).await.is_err() {
            return;
        }
        if stun && frames.send(frame).is_err() {
            return;
        }
    }
}

fn tls() -> io::Result<&'static tokio_rustls::TlsConnector> {
    static CONNECTOR: OnceLock<Result<tokio_rustls::TlsConnector, String>> = OnceLock::new();
    CONNECTOR
        .get_or_init(|| {
            use rustls_platform_verifier::BuilderVerifierExt as _;
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let config = rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .and_then(|b| b.with_platform_verifier())
                .map_err(|e| e.to_string())?
                .with_no_client_auth();
            Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
        })
        .as_ref()
        .map_err(|e| io::Error::other(e.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_cloudflare_ice_urls() {
        let parse = |url| {
            Server::parse(url, "u", "p").map(|(relay, s)| (relay, s.transport, s.host, s.port))
        };
        assert_eq!(
            parse("stun:stun.cloudflare.com:3478"),
            Some((false, Transport::Udp, "stun.cloudflare.com".into(), 3478))
        );
        assert_eq!(
            parse("turn:turn.cloudflare.com:3478?transport=tcp"),
            Some((true, Transport::Tcp, "turn.cloudflare.com".into(), 3478))
        );
        assert_eq!(
            parse("turns:turn.cloudflare.com:443?transport=tcp"),
            Some((true, Transport::Tls, "turn.cloudflare.com".into(), 443))
        );
        assert_eq!(
            parse("turn:[2001:db8::1]:3478"),
            Some((true, Transport::Udp, "2001:db8::1".into(), 3478))
        );
        assert_eq!(parse("https://example.com"), None);
    }
}
