use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::UdpSocket;

/// Where Minecraft listens for LAN worlds; the game shows each announcement under "LAN".
const GROUP: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::new(224, 0, 2, 60)), 4445);
const EVERY: Duration = Duration::from_millis(1500);

/// The message of a game log line in the vanilla, Fabric (`(Logger) msg`) or Forge
/// (`[logger]: msg`) layout; chat lines keep their `[CHAT]` prefix, so a player cannot fake one.
fn message(line: &str) -> Option<&str> {
    let (_, rest) = line.trim_end().strip_prefix('[')?.split_once("] [")?;
    let (_, rest) = rest.split_once(']')?;
    if let Some(message) = rest.strip_prefix(": ") {
        Some(message)
    } else if let Some(rest) = rest.strip_prefix(" (") {
        rest.split_once(") ").map(|(_, m)| m)
    } else {
        rest.strip_prefix(" [")?.split_once("]: ").map(|(_, m)| m)
    }
}

/// The port of a world just opened to LAN.
pub fn opened_port(line: &str) -> Option<u16> {
    let message = message(line)?;
    [
        "Published LAN server on port ",
        "Started serving on ",
        "Started on ",
    ]
    .iter()
    .find_map(|prefix| message.strip_prefix(prefix))?
    .parse()
    .ok()
}

/// Whether the world stopped being open to LAN, or the integrated server stopped.
pub fn stopped(line: &str) -> bool {
    message(line)
        .is_some_and(|m| m == "Stopping server" || m.starts_with("Unpublishing integrated server"))
}

/// Announces `port` as a LAN world named `motd` until the task is aborted.
pub fn announce(motd: String, port: u16) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // The game hears the group on one interface of its own choosing, and VPNs often
        // take the default route, so the world is announced on every interface.
        let sockets: Vec<UdpSocket> = crate::net::local_ips()
            .into_iter()
            .filter_map(|ip| match multicast_socket(ip) {
                Ok(socket) => Some(socket),
                Err(e) => {
                    tracing::debug!("lan: cannot announce on {ip}: {e}");
                    None
                }
            })
            .collect();
        // The game strips formatting codes but not brackets, which would end the MOTD early.
        let motd = motd.replace(['[', ']'], "");
        let packet = format!("[MOTD]{motd}[/MOTD][AD]{port}[/AD]");
        loop {
            for socket in &sockets {
                let _ = socket.send_to(packet.as_bytes(), GROUP).await;
            }
            tokio::time::sleep(EVERY).await;
        }
    })
}

fn multicast_socket(ip: Ipv4Addr) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.bind(&SocketAddr::new(ip.into(), 0).into())?;
    socket.set_multicast_if_v4(&ip)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_lan_port_of_old_and_new_games() {
        let opened = [
            (
                "[13:06:04] [Render thread/INFO]: Published LAN server on port 46249",
                46249,
            ),
            (
                "[12:01:33] [Server thread/INFO]: Started serving on 41721\r",
                41721,
            ),
            (
                "[12:01:33] [Server thread/INFO] (Minecraft) Started serving on 41721",
                41721,
            ),
            (
                "[12:01:33] [Server thread/INFO] [minecraft/IntegratedServer]: Started serving on 41721",
                41721,
            ),
            ("[12:01:33] [Server thread/INFO]: Started on 25565", 25565),
        ];
        for (line, port) in opened {
            assert_eq!(opened_port(line), Some(port), "{line}");
        }
        for chat in [
            "[13:06:04] [Render thread/INFO]: [CHAT] <bob> Published LAN server on port 22",
            "[13:06:04] [Render thread/INFO]: [System] [CHAT] This world is now open to LAN. The port number is [46249]",
        ] {
            assert_eq!(opened_port(chat), None, "{chat}");
        }
        assert!(stopped(
            "[13:06:23] [Render thread/INFO]: Unpublishing integrated server (was on port 46249)"
        ));
        assert!(stopped("[12:05:00] [Server thread/INFO]: Stopping server"));
        assert!(!stopped(
            "[12:05:00] [Render thread/INFO]: [CHAT] <bob> Stopping server"
        ));
    }
}
