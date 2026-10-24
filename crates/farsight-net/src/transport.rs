//! The byte transport under a session: TCP, optionally upgraded to TLS
//! (VeNCrypt) or wrapped in RA2 AES-EAX framing (RSA-AES).

use std::sync::{Arc, OnceLock};

use farsight_client::Fingerprint;
use farsight_security::ra2::Ra2Cipher;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tracing::debug;

enum Link {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
    Closed,
}

/// A connection to the server that may be upgraded mid-handshake.
pub(crate) struct Transport {
    link: Link,
    ra2: Option<Ra2Cipher>,
    framed: Vec<u8>,
}

/// What the TLS layer learned about the server's certificate.
pub(crate) struct TlsIdentity {
    /// SHA-256 of the leaf certificate.
    pub fingerprint: Fingerprint,
    /// Whether it chains to a trusted CA for the host name.
    pub valid_chain: bool,
}

impl Transport {
    pub(crate) fn new(stream: TcpStream) -> Self {
        Self {
            link: Link::Plain(stream),
            ra2: None,
            framed: Vec::new(),
        }
    }

    /// Reads and returns plaintext. `Ok(None)` means the peer closed the
    /// connection; an empty vector means a partial RA2 frame arrived.
    pub(crate) async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<Option<Vec<u8>>> {
        let n = match &mut self.link {
            Link::Plain(s) => s.read(buf).await?,
            Link::Tls(s) => s.read(buf).await?,
            Link::Closed => 0,
        };
        if n == 0 {
            return Ok(None);
        }
        let bytes = buf.get(..n).unwrap_or_default();
        let Some(cipher) = &mut self.ra2 else {
            return Ok(Some(bytes.to_vec()));
        };
        self.framed.extend_from_slice(bytes);
        let mut plain = Vec::new();
        let mut used = 0;
        while let Some((chunk, n)) = cipher
            .open(self.framed.get(used..).unwrap_or_default())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.reason))?
        {
            plain.extend(chunk);
            used += n;
        }
        self.framed.drain(..used);
        Ok(Some(plain))
    }

    /// Writes plaintext (framed when RA2 is active).
    pub(crate) async fn write(&mut self, plain: &[u8]) -> std::io::Result<()> {
        if plain.is_empty() {
            return Ok(());
        }
        let sealed;
        let bytes = match &mut self.ra2 {
            Some(cipher) => {
                let mut out = Vec::with_capacity(plain.len() + 64);
                cipher
                    .seal(plain, &mut out)
                    .map_err(|e| std::io::Error::other(e.reason))?;
                sealed = out;
                &sealed
            }
            None => plain,
        };
        match &mut self.link {
            Link::Plain(s) => s.write_all(bytes).await,
            Link::Tls(s) => s.write_all(bytes).await,
            Link::Closed => Err(std::io::ErrorKind::NotConnected.into()),
        }
    }

    /// Starts RA2 framing for everything sent and received from now on.
    pub(crate) fn enable_ra2(
        &mut self,
        send_key: &[u8],
        receive_key: &[u8],
    ) -> std::io::Result<()> {
        let cipher =
            Ra2Cipher::new(send_key, receive_key).map_err(|e| std::io::Error::other(e.reason))?;
        self.ra2 = Some(cipher);
        Ok(())
    }

    /// Runs a TLS handshake over the TCP connection and reports the server
    /// certificate. Signatures are verified; whether the certificate is
    /// *trusted* is decided by the session (CA chain or pinned fingerprint)
    /// before any credentials are sent (ADR-0017).
    pub(crate) async fn upgrade_tls(&mut self, host: &str) -> std::io::Result<TlsIdentity> {
        let Link::Plain(tcp) = std::mem::replace(&mut self.link, Link::Closed) else {
            return Err(std::io::Error::other(
                "TLS requested on a non-plain connection",
            ));
        };
        // Hosts that are neither a DNS name nor an IP can't match a CA
        // certificate anyway; the fingerprint check still applies.
        let server_name = ServerName::try_from(host.to_owned())
            .unwrap_or_else(|_| ServerName::IpAddress(std::net::IpAddr::from([0, 0, 0, 0]).into()));
        let tls = TlsConnector::from(client_config())
            .connect(server_name.clone(), tcp)
            .await?;
        let (_, connection) = tls.get_ref();
        let chain: Vec<CertificateDer<'static>> =
            connection.peer_certificates().unwrap_or_default().to_vec();
        let (leaf, intermediates) = chain
            .split_first()
            .ok_or_else(|| std::io::Error::other("server sent no certificate"))?;
        let fingerprint: Fingerprint = Sha256::digest(leaf.as_ref()).into();
        let valid_chain = webpki_verifier().is_some_and(|v| {
            v.verify_server_cert(leaf, intermediates, &server_name, &[], UnixTime::now())
                .is_ok()
        });
        debug!(valid_chain, "TLS established");
        self.link = Link::Tls(Box::new(tls));
        Ok(TlsIdentity {
            fingerprint,
            valid_chain,
        })
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn client_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let provider = provider();
        let config = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map(|builder| {
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(RecordingVerifier { provider }))
                    .with_no_client_auth()
            });
        Arc::new(config.unwrap_or_else(|_| {
            ClientConfig::builder()
                .with_root_certificates(RootCertStore::empty())
                .with_no_client_auth()
        }))
    }))
}

/// Verifies against the operating system's trusted CAs (None if none load).
fn webpki_verifier() -> Option<Arc<WebPkiServerVerifier>> {
    static VERIFIER: OnceLock<Option<Arc<WebPkiServerVerifier>>> = OnceLock::new();
    VERIFIER
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            for cert in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(cert);
            }
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
                .build()
                .ok()
        })
        .clone()
}

/// Accepts any certificate *for the handshake* but still checks handshake
/// signatures, so the server must hold the certificate's private key. Trust
/// in the certificate itself is decided afterwards from its fingerprint.
#[derive(Debug)]
struct RecordingVerifier {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for RecordingVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
