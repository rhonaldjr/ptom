//! Q2 acceptance tests for the active prober (ADR-0012, M3.5).
//!
//! Story: "As a user who marked this network as mine, I see every VNC server
//! on it", with the council condition that a probe never sends anything after
//! the security-type list: the only bytes a server receives are our 12-byte
//! `ProtocolVersion`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use farsight_discovery::{
    ProbeConfig, ProbeEvent, ProbeOutcome, ProbeProgress, ProbeScope, Provenance, Verification,
    probe, probe_one,
};
use farsight_rfb::server::{ServerConfig, ServerConnection};
use farsight_rfb::{Limits, ProtocolVersion, SecurityType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const WAIT: Duration = Duration::from_secs(5);

/// A one-connection RFB server driven by the real `ServerConnection` state
/// machine. Returns every byte the client sent before closing.
async fn fake_rfb_server(
    max_version: ProtocolVersion,
    security_types: Vec<SecurityType>,
) -> (SocketAddr, JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut connection = ServerConnection::new(ServerConfig {
            max_version,
            security_types,
            limits: Limits::DEFAULT,
        });
        stream.write_all(&connection.take_output()).await.unwrap();
        let mut received = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            let n = match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            received.extend_from_slice(&buffer[..n]);
            connection.receive(&buffer[..n]);
            while let Ok(Some(_)) = connection.poll() {}
            let reply = connection.take_output();
            if !reply.is_empty() && stream.write_all(&reply).await.is_err() {
                break;
            }
        }
        received
    });
    (addr, task)
}

/// A server that accepts one connection and runs `behaviour` on it.
async fn raw_server<F, Fut>(behaviour: F) -> SocketAddr
where
    F: FnOnce(TcpStream) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        behaviour(stream).await;
    });
    addr
}

fn quick_config() -> ProbeConfig {
    ProbeConfig {
        read_timeout: Duration::from_millis(300),
        ..ProbeConfig::default()
    }
}

#[tokio::test]
async fn lists_security_types_and_sends_only_the_version() {
    let (addr, server) = fake_rfb_server(
        ProtocolVersion::V3_8,
        vec![SecurityType::VeNCrypt, SecurityType::VncAuth],
    )
    .await;
    let scope = ProbeScope::from_hosts([LOCALHOST]);
    let config = ProbeConfig {
        ports: vec![addr.port()],
        ..ProbeConfig::default()
    };
    let (events, mut event_rx) = mpsc::unbounded_channel();

    let found = timeout(WAIT, probe(&scope, &config, events)).await.unwrap();

    assert_eq!(found.len(), 1);
    let server_entry = &found[0];
    assert_eq!(server_entry.provenance, Provenance::Probe);
    assert_eq!(server_entry.verification, Verification::Unverified);
    assert_eq!(server_entry.port, addr.port());
    assert_eq!(server_entry.addresses, vec![LOCALHOST]);
    let rfb = server_entry.rfb.as_ref().unwrap();
    assert_eq!(rfb.version, ProtocolVersion::V3_8);
    assert_eq!(
        rfb.security_types,
        vec![SecurityType::VeNCrypt, SecurityType::VncAuth]
    );
    assert!(rfb.offers_encryption);

    let received = timeout(WAIT, server).await.unwrap().unwrap();
    assert_eq!(
        received, b"RFB 003.008\n",
        "the probe must send its version and nothing after the security list"
    );

    let mut seen = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        seen.push(event);
    }
    assert_eq!(
        seen.first(),
        Some(&ProbeEvent::Progress(ProbeProgress {
            hosts_done: 0,
            hosts_total: 1
        }))
    );
    assert!(seen.contains(&ProbeEvent::Found(server_entry.clone())));
    assert_eq!(
        seen.last(),
        Some(&ProbeEvent::Progress(ProbeProgress {
            hosts_done: 1,
            hosts_total: 1
        }))
    );
}

#[tokio::test]
async fn answers_a_3_3_server_with_3_3() {
    let (addr, server) = fake_rfb_server(ProtocolVersion::V3_3, vec![SecurityType::VncAuth]).await;

    let outcome = probe_one(addr, &quick_config()).await;

    let ProbeOutcome::Rfb(rfb) = outcome else {
        panic!("expected an RFB server, got {outcome:?}");
    };
    assert_eq!(rfb.version, ProtocolVersion::V3_3);
    assert_eq!(rfb.security_types, vec![SecurityType::VncAuth]);
    assert!(!rfb.offers_encryption);
    let received = timeout(WAIT, server).await.unwrap().unwrap();
    assert_eq!(received, b"RFB 003.003\n");
}

#[tokio::test]
async fn legacy_only_server_is_not_encrypted() {
    let (addr, server) = fake_rfb_server(
        ProtocolVersion::V3_8,
        vec![SecurityType::VncAuth, SecurityType::None],
    )
    .await;

    let outcome = probe_one(addr, &quick_config()).await;

    let ProbeOutcome::Rfb(rfb) = outcome else {
        panic!("expected an RFB server, got {outcome:?}");
    };
    assert!(!rfb.offers_encryption);
    assert_eq!(
        timeout(WAIT, server).await.unwrap().unwrap(),
        b"RFB 003.008\n"
    );
}

#[tokio::test]
async fn refusing_server_is_still_listed() {
    // A server with no usable types sends a zero count and a reason.
    let (addr, server) = fake_rfb_server(ProtocolVersion::V3_8, Vec::new()).await;

    let outcome = probe_one(addr, &quick_config()).await;

    let ProbeOutcome::Rfb(rfb) = outcome else {
        panic!("expected an RFB server, got {outcome:?}");
    };
    assert_eq!(rfb.security_types, []);
    assert_eq!(
        timeout(WAIT, server).await.unwrap().unwrap(),
        b"RFB 003.008\n"
    );
}

#[tokio::test]
async fn silent_server_times_out() {
    let addr = raw_server(|stream| async move {
        tokio::time::sleep(WAIT).await;
        drop(stream);
    })
    .await;

    let started = std::time::Instant::now();
    let outcome = probe_one(addr, &quick_config()).await;

    assert_eq!(outcome, ProbeOutcome::ReadTimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn closed_port_fails_to_connect() {
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    assert_eq!(
        probe_one(addr, &quick_config()).await,
        ProbeOutcome::ConnectFailed
    );

    let scope = ProbeScope::from_hosts([LOCALHOST]);
    let config = ProbeConfig {
        ports: vec![addr.port()],
        ..quick_config()
    };
    let (events, _rx) = mpsc::unbounded_channel();
    assert_eq!(probe(&scope, &config, events).await, []);
}

#[tokio::test]
async fn garbage_is_not_rfb() {
    let addr = raw_server(|mut stream| async move {
        let _ = stream
            .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
            .await;
        tokio::time::sleep(WAIT).await;
    })
    .await;

    assert_eq!(probe_one(addr, &quick_config()).await, ProbeOutcome::NotRfb);
}

#[tokio::test]
async fn endless_stream_after_version_is_not_rfb() {
    // A valid banner, then a refusal claiming a 4 GiB reason and a flood of
    // bytes: the reader must stay bounded on hostile input.
    let addr = raw_server(|mut stream| async move {
        let _ = stream.write_all(b"RFB 003.008\n").await;
        let mut version = [0u8; 12];
        let _ = stream.read_exact(&mut version).await;
        // Refusal with an enormous claimed reason length.
        let mut reply = vec![0u8];
        reply.extend_from_slice(&u32::MAX.to_be_bytes());
        let _ = stream.write_all(&reply).await;
        let chunk = [b'x'; 4096];
        for _ in 0..64 {
            if stream.write_all(&chunk).await.is_err() {
                break;
            }
        }
    })
    .await;

    // The over-long reason is rejected by the farsight-rfb limits.
    assert_eq!(probe_one(addr, &quick_config()).await, ProbeOutcome::NotRfb);
}

#[tokio::test]
async fn early_close_is_reported() {
    let addr = raw_server(|stream| async move { drop(stream) }).await;
    assert_eq!(
        probe_one(addr, &quick_config()).await,
        ProbeOutcome::ClosedEarly
    );
}

#[tokio::test]
async fn dropping_the_probe_cancels_connections() {
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let addr = raw_server(|mut stream| async move {
        let mut buffer = [0u8; 64];
        // Never send a banner; wait until the prober goes away.
        let eof = timeout(WAIT, stream.read(&mut buffer)).await;
        let _ = closed_tx.send(matches!(eof, Ok(Ok(0) | Err(_))));
    })
    .await;
    let scope = ProbeScope::from_hosts([LOCALHOST]);
    let config = ProbeConfig {
        ports: vec![addr.port()],
        read_timeout: Duration::from_secs(30),
        ..ProbeConfig::default()
    };
    let (events, _rx) = mpsc::unbounded_channel();

    let cancelled = timeout(Duration::from_millis(200), probe(&scope, &config, events)).await;

    assert!(
        cancelled.is_err(),
        "the probe should still have been running"
    );
    assert!(
        timeout(WAIT, closed_rx).await.unwrap().unwrap(),
        "the connection should be closed once the probe is dropped"
    );
}

/// Opt-in interop: probe a real TigerVNC.
///
/// ```sh
/// docker build -t farsight-interop-tigervnc tests/interop/tigervnc
/// docker run -d --name fs-probe -p 127.0.0.1:15999:5901 farsight-interop-tigervnc
/// FARSIGHT_PROBE_ADDR=127.0.0.1:15999 cargo test -p farsight-discovery --test probe tigervnc
/// docker rm -f fs-probe
/// ```
#[tokio::test]
async fn tigervnc_interop_when_configured() {
    let Ok(addr) = std::env::var("FARSIGHT_PROBE_ADDR") else {
        eprintln!("FARSIGHT_PROBE_ADDR not set; skipping TigerVNC probe");
        return;
    };
    let addr: SocketAddr = addr.parse().expect("FARSIGHT_PROBE_ADDR must be host:port");

    // Docker's port proxy accepts before Xvnc listens; retry briefly.
    let mut outcome = ProbeOutcome::ClosedEarly;
    for _ in 0..20 {
        outcome = probe_one(addr, &ProbeConfig::default()).await;
        if matches!(outcome, ProbeOutcome::Rfb(_)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let ProbeOutcome::Rfb(rfb) = outcome else {
        panic!("expected TigerVNC to answer, got {outcome:?}");
    };
    eprintln!(
        "TigerVNC: version {:?}, types {:?}, encrypted {}",
        rfb.version, rfb.security_types, rfb.offers_encryption
    );
    assert_eq!(rfb.version, ProtocolVersion::V3_8);
    assert_ne!(rfb.security_types, []);
}
