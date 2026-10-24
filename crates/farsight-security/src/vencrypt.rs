//! VeNCrypt 0.2 (rfbproto "VeNCrypt"): subtype negotiation, then TLS, then
//! the subtype's own authentication inside TLS.
//!
//! Only the X509 subtypes are supported: they run TLS with a server
//! certificate the viewer can verify. The anonymous-TLS subtypes and plain
//! `Plain` are refused (ADR-0007).

use farsight_rfb::VeNCryptSubtype;
use farsight_rfb::auth::{
    AuthError, AuthProgress, AuthRequest, AuthResponse, Authenticator, TransportUpgrade,
};
use farsight_rfb::wire::{Reader, WriteExt};
use secrecy::{ExposeSecret, SecretString};

use crate::vnc_auth::{CHALLENGE_LEN, VncAuthClient, VncAuthServer};

/// Subtypes Farsight accepts, in the viewer's order of preference.
pub const SUPPORTED: [VeNCryptSubtype; 3] = [
    VeNCryptSubtype::X509None,
    VeNCryptSubtype::X509Plain,
    VeNCryptSubtype::X509Vnc,
];

/// Longest username or password accepted in a Plain exchange.
const MAX_CREDENTIAL_LEN: u32 = 1024;
/// Most subtypes accepted in the server's list.
const MAX_SUBTYPES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientState {
    Version,
    VersionAck,
    Subtypes,
    SubtypeAck,
    SubAuth,
}

/// The viewer side of VeNCrypt.
pub struct VeNCryptClient {
    state: ClientState,
    chosen: Option<VeNCryptSubtype>,
    username: Option<SecretString>,
    password: Option<SecretString>,
    vnc_auth: Option<VncAuthClient>,
}

impl VeNCryptClient {
    /// Creates the viewer side with credentials already known (if any).
    /// Missing ones are requested with [`AuthRequest::Credentials`] once the
    /// subtype shows they are needed.
    #[must_use]
    pub fn new(username: Option<SecretString>, password: Option<SecretString>) -> Self {
        Self {
            state: ClientState::Version,
            chosen: None,
            username,
            password,
            vnc_auth: None,
        }
    }

    /// The negotiated subtype, once chosen.
    #[must_use]
    pub fn subtype(&self) -> Option<VeNCryptSubtype> {
        self.chosen
    }

    fn sub_auth(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError> {
        match self.chosen {
            Some(VeNCryptSubtype::X509None) => Ok((0, AuthProgress::Complete)),
            Some(VeNCryptSubtype::X509Vnc) => {
                if let Some(inner) = &mut self.vnc_auth {
                    return inner.advance(input, output);
                }
                let Some(password) = self.password.clone() else {
                    return Ok((
                        0,
                        AuthProgress::Paused(AuthRequest::Credentials { username: false }),
                    ));
                };
                let inner = self.vnc_auth.insert(VncAuthClient::new(password));
                inner.advance(input, output)
            }
            Some(VeNCryptSubtype::X509Plain) => {
                let (Some(username), Some(password)) = (&self.username, &self.password) else {
                    return Ok((
                        0,
                        AuthProgress::Paused(AuthRequest::Credentials { username: true }),
                    ));
                };
                write_plain(output, username, password)?;
                Ok((0, AuthProgress::Complete))
            }
            _ => Err(AuthError::new("no VeNCrypt subtype negotiated")),
        }
    }
}

impl Authenticator for VeNCryptClient {
    fn advance(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError> {
        let mut used = 0;
        loop {
            let rest = input.get(used..).unwrap_or_default();
            let mut r = Reader::new(rest);
            match self.state {
                ClientState::Version => {
                    let Ok([major, minor]) = r.array::<2>() else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    if (major, minor) < (0, 2) {
                        return Err(AuthError::new(format!(
                            "server only supports VeNCrypt {major}.{minor}"
                        )));
                    }
                    output.extend_from_slice(&[0, 2]);
                    self.state = ClientState::VersionAck;
                }
                ClientState::VersionAck => {
                    let Ok(ack) = r.u8() else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    if ack != 0 {
                        return Err(AuthError::new("server rejected VeNCrypt 0.2"));
                    }
                    self.state = ClientState::Subtypes;
                }
                ClientState::Subtypes => {
                    let Some(offered) = read_subtypes(&mut r)? else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    let chosen = SUPPORTED.into_iter().find(|s| offered.contains(s)).ok_or_else(|| {
                        AuthError::new(format!(
                            "the server only offers VeNCrypt subtypes without server identity checks ({offered:?})"
                        ))
                    })?;
                    output.put_u32(chosen.code());
                    self.chosen = Some(chosen);
                    self.state = ClientState::SubtypeAck;
                }
                ClientState::SubtypeAck => {
                    let Ok(ack) = r.u8() else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    if ack != 1 {
                        return Err(AuthError::new("server rejected the VeNCrypt subtype"));
                    }
                    self.state = ClientState::SubAuth;
                    return Ok((
                        used + r.consumed(),
                        AuthProgress::UpgradeTransport(TransportUpgrade::Tls),
                    ));
                }
                ClientState::SubAuth => {
                    let (n, progress) = self.sub_auth(rest, output)?;
                    return Ok((used + n, progress));
                }
            }
            used += r.consumed();
        }
    }

    fn respond(&mut self, response: AuthResponse) -> Result<(), AuthError> {
        match response {
            AuthResponse::Credentials { username, password } => {
                self.username = username.or_else(|| self.username.take());
                self.password = Some(password);
                Ok(())
            }
            AuthResponse::ServerKeyAccepted => {
                Err(AuthError::new("unexpected server key confirmation"))
            }
        }
    }
}

fn read_subtypes(r: &mut Reader<'_>) -> Result<Option<Vec<VeNCryptSubtype>>, AuthError> {
    let Ok(count) = r.u8() else { return Ok(None) };
    let count = usize::from(count);
    if count == 0 || count > MAX_SUBTYPES {
        return Err(AuthError::new(
            "server sent an empty or oversized VeNCrypt subtype list",
        ));
    }
    let mut subtypes = Vec::with_capacity(count);
    for _ in 0..count {
        let Ok(code) = r.u32() else { return Ok(None) };
        subtypes.push(VeNCryptSubtype::from_code(code));
    }
    Ok(Some(subtypes))
}

fn write_plain(
    output: &mut Vec<u8>,
    username: &SecretString,
    password: &SecretString,
) -> Result<(), AuthError> {
    let user = username.expose_secret().as_bytes();
    let pass = password.expose_secret().as_bytes();
    let too_long = || AuthError::new("username or password is too long");
    output.put_u32(u32::try_from(user.len()).map_err(|_| too_long())?);
    output.put_u32(u32::try_from(pass.len()).map_err(|_| too_long())?);
    output.extend_from_slice(user);
    output.extend_from_slice(pass);
    Ok(())
}

/// Checks Plain (username + password) credentials on the agent.
pub trait PlainVerifier: Send {
    /// Whether the credentials are valid. Implementations must take the same
    /// time whether the user exists or not.
    fn verify(&self, username: &str, password: &SecretString) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerState {
    SendVersion,
    ClientVersion,
    Selection,
    SubAuth,
}

/// The agent side of VeNCrypt.
pub struct VeNCryptServer {
    state: ServerState,
    offered: Vec<VeNCryptSubtype>,
    chosen: Option<VeNCryptSubtype>,
    plain: Option<Box<dyn PlainVerifier>>,
    vnc_auth: Option<VncAuthServer>,
}

impl VeNCryptServer {
    /// Creates the agent side offering the X509 subtypes for which a verifier
    /// or password is supplied. `X509None` is offered only when
    /// `allow_unauthenticated` is set. `challenge` must be fresh randomness.
    #[must_use]
    pub fn new(
        plain: Option<Box<dyn PlainVerifier>>,
        vnc_password: Option<SecretString>,
        challenge: [u8; CHALLENGE_LEN],
        allow_unauthenticated: bool,
    ) -> Self {
        let mut offered = Vec::new();
        if plain.is_some() {
            offered.push(VeNCryptSubtype::X509Plain);
        }
        if vnc_password.is_some() {
            offered.push(VeNCryptSubtype::X509Vnc);
        }
        if allow_unauthenticated {
            offered.push(VeNCryptSubtype::X509None);
        }
        Self {
            state: ServerState::SendVersion,
            offered,
            chosen: None,
            plain,
            vnc_auth: vnc_password.map(|p| VncAuthServer::new(p, challenge)),
        }
    }

    fn sub_auth(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError> {
        match self.chosen {
            Some(VeNCryptSubtype::X509None) => Ok((0, AuthProgress::Complete)),
            Some(VeNCryptSubtype::X509Vnc) => match &mut self.vnc_auth {
                Some(inner) => inner.advance(input, output),
                None => Err(AuthError::new("VNC authentication not configured")),
            },
            Some(VeNCryptSubtype::X509Plain) => self.check_plain(input),
            _ => Err(AuthError::new("no VeNCrypt subtype negotiated")),
        }
    }

    fn check_plain(&self, input: &[u8]) -> Result<(usize, AuthProgress), AuthError> {
        let mut r = Reader::new(input);
        let (Ok(user_len), Ok(pass_len)) = (r.u32(), r.u32()) else {
            return Ok((0, AuthProgress::NeedMore));
        };
        if user_len > MAX_CREDENTIAL_LEN || pass_len > MAX_CREDENTIAL_LEN {
            return Err(AuthError::new("credentials too long"));
        }
        let (Ok(user), Ok(pass)) = (r.bytes(user_len as usize), r.bytes(pass_len as usize)) else {
            return Ok((0, AuthProgress::NeedMore));
        };
        let username = String::from_utf8_lossy(user);
        let password = SecretString::from(String::from_utf8_lossy(pass).into_owned());
        let verifier = self
            .plain
            .as_ref()
            .ok_or_else(|| AuthError::new("Plain authentication not configured"))?;
        if verifier.verify(&username, &password) {
            Ok((r.consumed(), AuthProgress::Complete))
        } else {
            Err(AuthError::new("authentication failed"))
        }
    }
}

impl Authenticator for VeNCryptServer {
    fn advance(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError> {
        let mut used = 0;
        loop {
            let rest = input.get(used..).unwrap_or_default();
            let mut r = Reader::new(rest);
            match self.state {
                ServerState::SendVersion => {
                    output.extend_from_slice(&[0, 2]);
                    self.state = ServerState::ClientVersion;
                }
                ServerState::ClientVersion => {
                    let Ok(version) = r.array::<2>() else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    if version != [0, 2] {
                        output.put_u8(0xff);
                        return Err(AuthError::new(
                            "client requested an unsupported VeNCrypt version",
                        ));
                    }
                    if self.offered.is_empty() {
                        output.put_u8(0xff);
                        return Err(AuthError::new("no VeNCrypt subtypes configured"));
                    }
                    output.put_u8(0);
                    output.put_u8(u8::try_from(self.offered.len()).unwrap_or(u8::MAX));
                    for subtype in &self.offered {
                        output.put_u32(subtype.code());
                    }
                    self.state = ServerState::Selection;
                }
                ServerState::Selection => {
                    let Ok(code) = r.u32() else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    let chosen = VeNCryptSubtype::from_code(code);
                    if !self.offered.contains(&chosen) {
                        output.put_u8(0);
                        return Err(AuthError::new(
                            "client chose a VeNCrypt subtype that was not offered",
                        ));
                    }
                    output.put_u8(1);
                    self.chosen = Some(chosen);
                    self.state = ServerState::SubAuth;
                    return Ok((
                        used + r.consumed(),
                        AuthProgress::UpgradeTransport(TransportUpgrade::Tls),
                    ));
                }
                ServerState::SubAuth => {
                    let (n, progress) = self.sub_auth(rest, output)?;
                    return Ok((used + n, progress));
                }
            }
            used += r.consumed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct OnlyAlice;
    impl PlainVerifier for OnlyAlice {
        fn verify(&self, username: &str, password: &SecretString) -> bool {
            username == "alice" && password.expose_secret() == "wonderland"
        }
    }

    fn secret(s: &str) -> SecretString {
        SecretString::from(s.to_owned())
    }

    /// Runs both sides to completion, treating transport upgrades as no-ops
    /// and answering credential requests from `answer`.
    fn exchange(
        client: &mut VeNCryptClient,
        server: &mut VeNCryptServer,
        answer: Option<(Option<&str>, &str)>,
    ) -> Result<(), AuthError> {
        let (mut to_client, mut to_server) = (Vec::new(), Vec::new());
        let (mut client_done, mut server_done) = (false, false);
        let mut paused = false;
        for _ in 0..20 {
            if !server_done {
                let (n, p) = server.advance(&to_server, &mut to_client)?;
                to_server.drain(..n);
                server_done = p == AuthProgress::Complete;
            }
            if !client_done {
                let (n, p) = client.advance(&to_client, &mut to_server)?;
                to_client.drain(..n);
                match p {
                    AuthProgress::Complete => client_done = true,
                    AuthProgress::Paused(AuthRequest::Credentials { .. }) => {
                        paused = true;
                        let (user, pass) = answer.ok_or_else(|| AuthError::new("no answer"))?;
                        client.respond(AuthResponse::Credentials {
                            username: user.map(secret),
                            password: secret(pass),
                        })?;
                    }
                    _ => {}
                }
            }
            if client_done && server_done {
                assert_eq!(answer.is_some(), paused);
                return Ok(());
            }
        }
        Err(AuthError::new("did not finish"))
    }

    #[test]
    fn x509_plain_with_prompted_credentials() {
        let mut client = VeNCryptClient::new(None, None);
        let mut server = VeNCryptServer::new(Some(Box::new(OnlyAlice)), None, [0; 16], false);
        exchange(
            &mut client,
            &mut server,
            Some((Some("alice"), "wonderland")),
        )
        .unwrap();
        assert_eq!(client.subtype(), Some(VeNCryptSubtype::X509Plain));
    }

    #[test]
    fn x509_plain_wrong_password_fails_on_the_server() {
        let mut client = VeNCryptClient::new(Some(secret("alice")), Some(secret("nope")));
        let mut server = VeNCryptServer::new(Some(Box::new(OnlyAlice)), None, [0; 16], false);
        assert!(exchange(&mut client, &mut server, None).is_err());
    }

    #[test]
    fn x509_vnc_inside_tls() {
        let mut client = VeNCryptClient::new(None, Some(secret("pw")));
        let mut server = VeNCryptServer::new(None, Some(secret("pw")), [5; 16], false);
        exchange(&mut client, &mut server, None).unwrap();
        assert_eq!(client.subtype(), Some(VeNCryptSubtype::X509Vnc));
    }

    #[test]
    fn x509_none_needs_no_credentials() {
        let mut client = VeNCryptClient::new(None, None);
        let mut server = VeNCryptServer::new(Some(Box::new(OnlyAlice)), None, [0; 16], true);
        exchange(&mut client, &mut server, None).unwrap();
        assert_eq!(client.subtype(), Some(VeNCryptSubtype::X509None));
    }

    #[test]
    fn client_refuses_anonymous_tls_only_servers() {
        let mut client = VeNCryptClient::new(None, None);
        let mut out = Vec::new();
        // Version 0.2, ack, then two anonymous subtypes.
        let input = [0, 2, 0, 2, 0, 0, 1, 1, 0, 0, 1, 2];
        let error = client.advance(&input, &mut out).unwrap_err();
        assert!(error.reason.contains("without server identity checks"));
    }

    #[test]
    fn client_upgrades_to_tls_after_the_subtype_ack() {
        let mut client = VeNCryptClient::new(None, None);
        let mut out = Vec::new();
        let input = [0, 2, 0, 1, 0, 0, 1, 4, 1];
        assert_eq!(
            client.advance(&input, &mut out).unwrap(),
            (
                input.len(),
                AuthProgress::UpgradeTransport(TransportUpgrade::Tls)
            )
        );
        assert_eq!(out, [0, 2, 0, 0, 1, 4]);
    }

    #[test]
    fn client_input_is_handled_incrementally() {
        let input = [0u8, 2, 0, 1, 0, 0, 1, 4, 1];
        let mut client = VeNCryptClient::new(None, None);
        let mut pending = Vec::new();
        let mut out = Vec::new();
        for &byte in &input {
            pending.push(byte);
            let (n, progress) = client.advance(&pending, &mut out).unwrap();
            pending.drain(..n);
            if progress == AuthProgress::UpgradeTransport(TransportUpgrade::Tls) {
                assert_eq!(out, [0, 2, 0, 0, 1, 4]);
                return;
            }
        }
        panic!("never upgraded");
    }

    #[test]
    fn server_rejects_unoffered_choice_and_oversized_plain() {
        let mut server = VeNCryptServer::new(Some(Box::new(OnlyAlice)), None, [0; 16], false);
        let mut out = Vec::new();
        server.advance(&[], &mut out).unwrap();
        server.advance(&[0, 2], &mut out).unwrap();
        assert!(server.advance(&260u32.to_be_bytes(), &mut out).is_err());

        let mut server = VeNCryptServer::new(Some(Box::new(OnlyAlice)), None, [0; 16], false);
        server.advance(&[], &mut out).unwrap();
        server.advance(&[0, 2], &mut out).unwrap();
        server.advance(&262u32.to_be_bytes(), &mut out).unwrap();
        let mut huge = Vec::new();
        huge.put_u32(1 << 20);
        huge.put_u32(1);
        assert!(server.advance(&huge, &mut out).is_err());
    }
}
