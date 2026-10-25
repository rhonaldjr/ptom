//! A minimal HTTPS GET (rustls with the aws-lc provider and the system's
//! root certificates), used only by the explicit install command.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use crate::H264Error;

/// Largest response accepted (Cisco's file is well under 1 MiB).
const MAX_RESPONSE: u64 = 16 << 20;
const TIMEOUT: Duration = Duration::from_secs(30);

fn error(e: impl std::fmt::Display) -> H264Error {
    H264Error::Download(e.to_string())
}

pub(crate) fn https_get(host: &str, path: &str) -> Result<Vec<u8>, H264Error> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(error)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(error)?;
    let connection = rustls::ClientConnection::new(Arc::new(config), name).map_err(error)?;
    let tcp = TcpStream::connect((host, 443)).map_err(error)?;
    tcp.set_read_timeout(Some(TIMEOUT))?;
    tcp.set_write_timeout(Some(TIMEOUT))?;
    let mut tls = rustls::StreamOwned::new(connection, tcp);
    write!(
        tls,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: farsight\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = Vec::new();
    match Read::by_ref(&mut tls)
        .take(MAX_RESPONSE)
        .read_to_end(&mut response)
    {
        Ok(_) => {}
        // Servers often close without TLS close_notify after `Connection: close`.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !response.is_empty() => {}
        Err(e) => return Err(e.into()),
    }
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| error("no HTTP headers"))?;
    let (head, body) = response.split_at(split + 4);
    let head = String::from_utf8_lossy(head);
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return Err(error(head.lines().next().unwrap_or("bad response")));
    }
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        return Err(error("chunked responses aren't supported"));
    }
    Ok(body.to_vec())
}
