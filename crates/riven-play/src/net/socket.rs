use std::collections::HashMap;
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};

use bytes::Bytes;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use super::turn::Allocation;

/// How a datagram leaves this side: straight from the UDP socket or through one of its relays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    Direct,
    Relay(usize),
}

#[derive(Default)]
struct Table {
    by_virtual: HashMap<SocketAddr, (Route, SocketAddr)>,
    by_real: HashMap<(Route, SocketAddr), SocketAddr>,
}

/// Gives every route and peer address pair its own address, so quinn treats each as a path.
#[derive(Default)]
pub struct Paths(Mutex<Table>);

impl Paths {
    pub fn virtual_addr(&self, route: Route, real: SocketAddr) -> SocketAddr {
        let mut table = self.0.lock().expect("paths lock");
        if let Some(addr) = table.by_real.get(&(route, real)) {
            return *addr;
        }
        let n = table.by_virtual.len() as u32 + 1;
        let addr = SocketAddr::new(
            Ipv6Addr::new(0xfd52, 0x6976, 0x656e, 0, 0, 0, (n >> 16) as u16, n as u16).into(),
            real.port(),
        );
        table.by_virtual.insert(addr, (route, real));
        table.by_real.insert((route, real), addr);
        addr
    }

    pub fn real(&self, addr: SocketAddr) -> Option<(Route, SocketAddr)> {
        self.0
            .lock()
            .expect("paths lock")
            .by_virtual
            .get(&addr)
            .copied()
    }
}

pub type Inbox = mpsc::UnboundedSender<(SocketAddr, Bytes)>;

/// The socket quinn runs on: datagrams to virtual addresses go out by their route.
pub struct VirtualSocket {
    pub direct: Arc<UdpSocket>,
    pub relays: OnceLock<Vec<Option<Allocation>>>,
    pub paths: Arc<Paths>,
    inbox: Mutex<mpsc::UnboundedReceiver<(SocketAddr, Bytes)>>,
    /// Keeps the inbox open after the readers stop, so quinn sees a quiet socket, not a broken one.
    _inbox: Inbox,
}

impl VirtualSocket {
    pub fn new(direct: Arc<UdpSocket>, paths: Arc<Paths>) -> (Self, Inbox) {
        let (tx, rx) = mpsc::unbounded_channel();
        let socket = Self {
            direct,
            relays: OnceLock::new(),
            paths,
            inbox: Mutex::new(rx),
            _inbox: tx.clone(),
        };
        (socket, tx)
    }

    /// Sends outside quinn, for hole punching.
    pub fn send_raw(&self, route: Route, to: SocketAddr, data: &[u8]) {
        match route {
            Route::Direct => {
                let _ = self.direct.try_send_to(data, to);
            }
            Route::Relay(i) => {
                if let Some(relay) = self
                    .relays
                    .get()
                    .and_then(|r| r.get(i))
                    .and_then(Option::as_ref)
                {
                    relay.send(to, data);
                }
            }
        }
    }
}

impl fmt::Debug for VirtualSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualSocket").finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct AlwaysWritable;

impl UdpPoller for AlwaysWritable {
    fn poll_writable(self: Pin<&mut Self>, _: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncUdpSocket for VirtualSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(AlwaysWritable)
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        // A datagram that cannot go now is lost, as on any UDP socket; QUIC sends it again.
        if let Some((route, real)) = self.paths.real(transmit.destination) {
            self.send_raw(route, real, transmit.contents);
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut inbox = self.inbox.lock().expect("inbox lock");
        let mut n = 0;
        while n < bufs.len().min(meta.len()) {
            match inbox.poll_recv(cx) {
                Poll::Ready(Some((from, data))) => {
                    let len = data.len().min(bufs[n].len());
                    bufs[n][..len].copy_from_slice(&data[..len]);
                    meta[n] = RecvMeta {
                        addr: from,
                        len,
                        stride: len,
                        ecn: None,
                        dst_ip: None,
                    };
                    n += 1;
                }
                Poll::Ready(None) | Poll::Pending => break,
            }
        }
        if n == 0 {
            Poll::Pending
        } else {
            Poll::Ready(Ok(n))
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::new(
            Ipv6Addr::new(0xfd52, 0x6976, 0x656e, 0, 0, 0, 0, 0).into(),
            1,
        ))
    }

    fn may_fragment(&self) -> bool {
        false
    }
}
