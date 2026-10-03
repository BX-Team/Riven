use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

const PREFIX: &str = "ed25519:";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SignError {
    #[error("not an ed25519 key (expected `ed25519:` and 32 base64url bytes)")]
    BadKey,
    #[error("malformed signature")]
    BadSignature,
    #[error("signature does not match")]
    Mismatch,
    #[error("no system randomness: {0}")]
    Random(String),
}

fn decode<const N: usize>(text: &str) -> Option<[u8; N]> {
    URL_SAFE_NO_PAD.decode(text.trim()).ok()?.try_into().ok()
}

/// An author's signing key; its text form is the secret seed.
pub struct KeyPair(SigningKey);

impl KeyPair {
    pub fn generate() -> Result<Self, SignError> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| SignError::Random(e.to_string()))?;
        Ok(Self(SigningKey::from_bytes(&seed)))
    }

    /// Parses [`KeyPair::to_secret`] output.
    pub fn from_secret(text: &str) -> Result<Self, SignError> {
        let seed = text
            .trim()
            .strip_prefix(PREFIX)
            .and_then(decode::<32>)
            .ok_or(SignError::BadKey)?;
        Ok(Self(SigningKey::from_bytes(&seed)))
    }

    pub fn to_secret(&self) -> String {
        format!("{PREFIX}{}\n", URL_SAFE_NO_PAD.encode(self.0.to_bytes()))
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }

    /// A detached signature of `bytes`, as the text of a `.sig` file.
    pub fn sign(&self, bytes: &[u8]) -> String {
        format!(
            "{}\n",
            URL_SAFE_NO_PAD.encode(self.0.sign(bytes).to_bytes())
        )
    }
}

/// A public key as install links and `trusted.json` carry it: `ed25519:<base64url>`.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    pub fn parse(text: &str) -> Result<Self, SignError> {
        let bytes = text
            .trim()
            .strip_prefix(PREFIX)
            .and_then(decode::<32>)
            .ok_or(SignError::BadKey)?;
        VerifyingKey::from_bytes(&bytes)
            .map(Self)
            .map_err(|_| SignError::BadKey)
    }

    /// Checks a `.sig` file's text against `bytes`.
    pub fn verify(&self, bytes: &[u8], signature: &str) -> Result<(), SignError> {
        let raw = decode::<64>(signature).ok_or(SignError::BadSignature)?;
        self.0
            .verify_strict(bytes, &Signature::from_bytes(&raw))
            .map_err(|_| SignError::Mismatch)
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PREFIX}{}", URL_SAFE_NO_PAD.encode(self.0.to_bytes()))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_round_trip_and_reject_tampering() {
        let key = KeyPair::generate().unwrap();
        let again = KeyPair::from_secret(&key.to_secret()).unwrap();
        assert_eq!(again.public(), key.public());

        let public = PublicKey::parse(&key.public().to_string()).unwrap();
        let sig = key.sign(b"{\"version\":\"1.4.0\"}");
        assert_eq!(public.verify(b"{\"version\":\"1.4.0\"}", &sig), Ok(()));
        assert_eq!(
            public.verify(b"{\"version\":\"1.4.1\"}", &sig),
            Err(SignError::Mismatch)
        );
        let other = KeyPair::generate().unwrap();
        assert_eq!(
            other.public().verify(b"{\"version\":\"1.4.0\"}", &sig),
            Err(SignError::Mismatch)
        );
        assert_eq!(public.verify(b"x", "garbage"), Err(SignError::BadSignature));
        assert_eq!(PublicKey::parse("ed25519:AAAA"), Err(SignError::BadKey));
        assert_eq!(PublicKey::parse("rsa:abc"), Err(SignError::BadKey));
    }
}
