use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use hmac::{KeyInit as _, Mac as _};

pub const MAGIC: u32 = 0x2112_a442;
const HEADER: usize = 20;

pub const BINDING_REQUEST: u16 = 0x0001;
pub const BINDING_SUCCESS: u16 = 0x0101;
pub const BINDING_INDICATION: u16 = 0x0011;
pub const ALLOCATE: u16 = 0x0003;
pub const REFRESH: u16 = 0x0004;
pub const CREATE_PERMISSION: u16 = 0x0008;
pub const SEND_INDICATION: u16 = 0x0016;
pub const DATA_INDICATION: u16 = 0x0017;

pub const USERNAME: u16 = 0x0006;
pub const MESSAGE_INTEGRITY: u16 = 0x0008;
pub const ERROR_CODE: u16 = 0x0009;
pub const LIFETIME: u16 = 0x000d;
pub const XOR_PEER_ADDRESS: u16 = 0x0012;
pub const DATA: u16 = 0x0013;
pub const REALM: u16 = 0x0014;
pub const NONCE: u16 = 0x0015;
pub const XOR_RELAYED_ADDRESS: u16 = 0x0016;
pub const REQUESTED_TRANSPORT: u16 = 0x0019;
pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// The method bits of a message type, without its class.
pub fn method(kind: u16) -> u16 {
    kind & !0x0110
}

pub fn is_success(kind: u16) -> bool {
    kind & 0x0110 == 0x0100
}

pub fn is_error(kind: u16) -> bool {
    kind & 0x0110 == 0x0110
}

/// Whether a datagram is STUN rather than QUIC: QUIC always sets the second-highest bit.
pub fn is_stun(buf: &[u8]) -> bool {
    buf.len() >= HEADER && buf[0] & 0xc0 == 0 && buf[4..8] == MAGIC.to_be_bytes()
}

/// The full length of the STUN message `head` starts, once its first four bytes are in.
pub fn framed_len(head: &[u8; 4]) -> usize {
    HEADER + u16::from_be_bytes([head[2], head[3]]) as usize
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub kind: u16,
    pub tid: [u8; 12],
    pub attrs: Vec<(u16, Vec<u8>)>,
}

impl Message {
    pub fn new(kind: u16) -> Self {
        let mut tid = [0; 12];
        let _ = getrandom::fill(&mut tid);
        Self {
            kind,
            tid,
            attrs: Vec::new(),
        }
    }

    pub fn with(mut self, kind: u16, value: impl Into<Vec<u8>>) -> Self {
        self.attrs.push((kind, value.into()));
        self
    }

    pub fn with_addr(self, kind: u16, addr: SocketAddr) -> Self {
        let value = xor_addr(addr, &self.tid);
        self.with(kind, value)
    }

    pub fn attr(&self, kind: u16) -> Option<&[u8]> {
        self.attrs
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, v)| v.as_slice())
    }

    pub fn addr(&self, kind: u16) -> Option<SocketAddr> {
        unxor_addr(self.attr(kind)?, &self.tid)
    }

    pub fn error_code(&self) -> Option<u16> {
        let v = self.attr(ERROR_CODE)?;
        (v.len() >= 4).then(|| u16::from(v[2] & 0x07) * 100 + u16::from(v[3]))
    }

    /// Encodes the message, signed with MESSAGE-INTEGRITY when a key is given.
    pub fn encode(&self, key: Option<&[u8]>) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER + 64);
        buf.extend_from_slice(&self.kind.to_be_bytes());
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&MAGIC.to_be_bytes());
        buf.extend_from_slice(&self.tid);
        for (kind, value) in &self.attrs {
            put_attr(&mut buf, *kind, value);
        }
        if let Some(key) = key {
            // The length covers the integrity attribute itself, which is not written yet.
            let len = buf.len() - HEADER + 24;
            set_len(&mut buf, len);
            let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(key).expect("any key length");
            mac.update(&buf);
            put_attr(&mut buf, MESSAGE_INTEGRITY, &mac.finalize().into_bytes());
        }
        let len = buf.len() - HEADER;
        set_len(&mut buf, len);
        buf
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if !is_stun(buf) || framed_len(buf[..4].try_into().ok()?) != buf.len() {
            return None;
        }
        let kind = u16::from_be_bytes([buf[0], buf[1]]);
        let tid = buf[8..20].try_into().ok()?;
        let mut attrs = Vec::new();
        let mut rest = &buf[HEADER..];
        while rest.len() >= 4 {
            let attr = u16::from_be_bytes([rest[0], rest[1]]);
            let len = u16::from_be_bytes([rest[2], rest[3]]) as usize;
            let value = rest.get(4..4 + len)?;
            attrs.push((attr, value.to_vec()));
            rest = rest
                .get((4 + len).next_multiple_of(4)..)
                .unwrap_or_default();
        }
        Some(Self { kind, tid, attrs })
    }
}

/// Whether a raw message carries a MESSAGE-INTEGRITY made with `key`.
pub fn verify(raw: &[u8], key: &[u8]) -> bool {
    let mut at = HEADER;
    while at + 4 <= raw.len() {
        let kind = u16::from_be_bytes([raw[at], raw[at + 1]]);
        let len = u16::from_be_bytes([raw[at + 2], raw[at + 3]]) as usize;
        if kind == MESSAGE_INTEGRITY {
            let Some(expected) = raw.get(at + 4..at + 4 + 20) else {
                return false;
            };
            let mut signed = raw[..at].to_vec();
            set_len(&mut signed, at - HEADER + 24);
            let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(key).expect("any key length");
            mac.update(&signed);
            return mac.verify_slice(expected).is_ok();
        }
        at += (4 + len).next_multiple_of(4);
    }
    false
}

fn put_attr(buf: &mut Vec<u8>, kind: u16, value: &[u8]) {
    buf.extend_from_slice(&kind.to_be_bytes());
    buf.extend_from_slice(&(value.len() as u16).to_be_bytes());
    buf.extend_from_slice(value);
    buf.resize(buf.len().next_multiple_of(4), 0);
}

fn set_len(buf: &mut [u8], len: usize) {
    buf[2..4].copy_from_slice(&(len as u16).to_be_bytes());
}

fn xor_addr(addr: SocketAddr, tid: &[u8; 12]) -> Vec<u8> {
    let port = addr.port() ^ (MAGIC >> 16) as u16;
    let mut out = vec![0];
    match addr.ip() {
        IpAddr::V4(ip) => {
            out.push(1);
            out.extend_from_slice(&port.to_be_bytes());
            out.extend_from_slice(&(u32::from(ip) ^ MAGIC).to_be_bytes());
        }
        IpAddr::V6(ip) => {
            out.push(2);
            out.extend_from_slice(&port.to_be_bytes());
            let mask = v6_mask(tid);
            out.extend(ip.octets().iter().zip(mask).map(|(a, b)| a ^ b));
        }
    }
    out
}

fn unxor_addr(v: &[u8], tid: &[u8; 12]) -> Option<SocketAddr> {
    let port = u16::from_be_bytes([*v.get(2)?, *v.get(3)?]) ^ (MAGIC >> 16) as u16;
    let ip = match v.get(1)? {
        1 => {
            let raw = u32::from_be_bytes(v.get(4..8)?.try_into().ok()?);
            IpAddr::V4(Ipv4Addr::from(raw ^ MAGIC))
        }
        2 => {
            let raw: [u8; 16] = v.get(4..20)?.try_into().ok()?;
            let mut octets = [0; 16];
            for (o, (a, b)) in octets.iter_mut().zip(raw.iter().zip(v6_mask(tid))) {
                *o = a ^ b;
            }
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

fn v6_mask(tid: &[u8; 12]) -> [u8; 16] {
    let mut mask = [0; 16];
    mask[..4].copy_from_slice(&MAGIC.to_be_bytes());
    mask[4..].copy_from_slice(tid);
    mask
}

/// The long-term credential key of RFC 5389: MD5 of `username:realm:password`.
pub fn long_term_key(username: &str, realm: &str, password: &str) -> Vec<u8> {
    use md5::Digest as _;
    md5::Md5::digest(format!("{username}:{realm}:{password}")).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc5769_response_verifies_and_addresses_round_trip() {
        let raw = hex::decode(concat!(
            "0101003c2112a442b7e7a701bc34d686fa87dfae",
            "8022000b7465737420766563746f7220",
            "002000080001a147e112a643",
            "000800142b91f599fd9e90c38c7489f92af9ba53f06be7d7",
            "80280004c07d4c96",
        ))
        .unwrap();
        assert!(verify(&raw, b"VOkJxbRl1RmTxUk/WvJxBt"));
        assert!(!verify(&raw, b"wrong"));
        let response = Message::decode(&raw).unwrap();
        assert_eq!(
            response.addr(XOR_MAPPED_ADDRESS),
            Some("192.0.2.1:32853".parse().unwrap())
        );

        let key = long_term_key("user", "realm", "pass");
        for addr in [
            "192.0.2.1:32853",
            "[2001:db8:1234:5678:11:2233:4455:6677]:32853",
        ] {
            let addr: SocketAddr = addr.parse().unwrap();
            let raw = Message::new(BINDING_SUCCESS)
                .with_addr(XOR_MAPPED_ADDRESS, addr)
                .with(USERNAME, "user")
                .encode(Some(&key));
            assert!(verify(&raw, &key));
            assert_eq!(
                Message::decode(&raw).unwrap().addr(XOR_MAPPED_ADDRESS),
                Some(addr)
            );
        }
    }
}
