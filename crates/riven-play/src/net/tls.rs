use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, Error, SignatureScheme};
use sha2::Digest as _;

const ALPN: &[u8] = b"riven-play/1";
pub const SERVER_NAME: &str = "riven";

pub type Fingerprint = [u8; 32];

/// Guests a host lets in, by certificate fingerprint; filled from their offers.
pub type Allowed = Arc<Mutex<HashSet<Fingerprint>>>;

pub fn fingerprint(cert: &[u8]) -> Fingerprint {
    sha2::Sha256::digest(cert).into()
}

/// A throwaway certificate per session; peers learn its fingerprint through the lobby.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: Vec<u8>,
    pub fingerprint: Fingerprint,
}

impl Identity {
    pub fn generate() -> Result<Self, String> {
        let made = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_owned()])
            .map_err(|e| e.to_string())?;
        let cert = made.cert.der().clone();
        Ok(Self {
            fingerprint: fingerprint(&cert),
            cert,
            key: made.signing_key.serialize_der(),
        })
    }

    fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()))
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config.keep_alive_interval(Some(Duration::from_secs(5)));
    config.max_idle_timeout(Some(
        Duration::from_secs(30).try_into().expect("idle timeout"),
    ));
    Arc::new(config)
}

pub fn server_config(identity: &Identity, allowed: Allowed) -> Result<quinn::ServerConfig, String> {
    let provider = provider();
    let verifier = Arc::new(PinnedClients {
        allowed,
        algorithms: provider.signature_verification_algorithms,
    });
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![identity.cert.clone()], identity.key())
        .map_err(|e| e.to_string())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto =
        quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(|e| e.to_string())?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(transport());
    Ok(config)
}

/// Connects only to the peer whose certificate has `expected` as its fingerprint.
pub fn client_config(
    identity: &Identity,
    expected: Fingerprint,
) -> Result<quinn::ClientConfig, String> {
    let provider = provider();
    let verifier = Arc::new(PinnedServer {
        expected,
        algorithms: provider.signature_verification_algorithms,
    });
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![identity.cert.clone()], identity.key())
        .map_err(|e| e.to_string())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto =
        quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(|e| e.to_string())?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport());
    Ok(config)
}

/// The fingerprint of the certificate a connected peer showed.
pub fn peer_fingerprint(connection: &quinn::Connection) -> Option<Fingerprint> {
    let certs = connection
        .peer_identity()?
        .downcast::<Vec<CertificateDer<'static>>>()
        .ok()?;
    certs.first().map(|c| fingerprint(c))
}

fn unknown() -> Error {
    Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
}

#[derive(Debug)]
struct PinnedServer {
    expected: Fingerprint,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if fingerprint(end_entity) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(unknown())
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[derive(Debug)]
struct PinnedClients {
    allowed: Allowed,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for PinnedClients {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        let known = self
            .allowed
            .lock()
            .is_ok_and(|a| a.contains(&fingerprint(end_entity)));
        if known {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(unknown())
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
