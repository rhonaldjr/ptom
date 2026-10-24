//! RSA-AES security types (rfbproto "RSA-AES Security Type"): RA2 (SHA-1,
//! AES-128) and RA2_256 (SHA-256, AES-256).
//!
//! The handshake exchanges RSA public keys and RSA-encrypted random numbers,
//! derives two session keys, and then frames **all** further traffic in
//! AES-EAX ([`Ra2Cipher`]), applied by the transport after
//! [`TransportUpgrade::Ra2`]. RSA uses aws-lc-rs, whose PKCS#1 v1.5
//! decryption is constant-time (the pure-Rust `rsa` crate is affected by the
//! Marvin attack, RUSTSEC-2023-0071; see ADR-0016).

use aes::{Aes128, Aes256};
use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use aws_lc_rs::rsa::{
    KeySize, Pkcs1PrivateDecryptingKey, Pkcs1PublicEncryptingKey, PrivateDecryptingKey,
    PublicEncryptingKey,
};
use eax::Eax;
use eax::aead::generic_array::GenericArray;
use eax::aead::{AeadInPlace, KeyInit};
use farsight_rfb::SecurityType;
use farsight_rfb::auth::{
    AuthError, AuthProgress, AuthRequest, AuthResponse, Authenticator, TransportUpgrade,
};
use farsight_rfb::wire::{Reader, WriteExt};
use secrecy::{ExposeSecret, SecretString};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::der;
use crate::vencrypt::PlainVerifier;

/// Smallest and largest peer RSA keys accepted, in bits.
const MIN_KEY_BITS: u32 = 2048;
const MAX_KEY_BITS: u32 = 8192;
/// Largest plaintext per AES-EAX frame we send.
const MAX_FRAME: usize = 8192;
const TAG_LEN: usize = 16;
/// Subtype values sent by the server.
const SUBTYPE_USER_PASSWORD: u8 = 1;
const SUBTYPE_PASSWORD: u8 = 2;

fn crypto_error(_: impl std::fmt::Debug) -> AuthError {
    AuthError::new("RSA-AES cryptographic operation failed")
}

/// Which RSA-AES flavour is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ra2Variant {
    /// `RA2`: SHA-1 hashes, AES-128-EAX.
    Sha1,
    /// `RA2_256`: SHA-256 hashes, AES-256-EAX.
    Sha256,
}

impl Ra2Variant {
    /// The variant for an encrypted RSA-AES security type. The "ne" types,
    /// which drop encryption after authentication, are not supported.
    #[must_use]
    pub fn for_security_type(security: SecurityType) -> Option<Self> {
        match security {
            SecurityType::Ra2 => Some(Self::Sha1),
            SecurityType::Ra256 => Some(Self::Sha256),
            _ => None,
        }
    }

    fn hash(self, first: &[u8], second: &[u8]) -> Vec<u8> {
        match self {
            Self::Sha1 => Sha1::new()
                .chain_update(first)
                .chain_update(second)
                .finalize()
                .to_vec(),
            Self::Sha256 => Sha256::new()
                .chain_update(first)
                .chain_update(second)
                .finalize()
                .to_vec(),
        }
    }

    /// Bytes of randomness each side contributes. The spec text says 16 for
    /// every variant, but deployed servers (observed black-box against
    /// TigerVNC) require 32 for RA2_256, matching the AES-256 key size.
    fn random_len(self) -> usize {
        self.key_len()
    }

    fn key_len(self) -> usize {
        match self {
            Self::Sha1 => 16,
            Self::Sha256 => 32,
        }
    }

    /// (client session key, server session key).
    fn session_keys(self, server_random: &[u8], client_random: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut client = self.hash(server_random, client_random);
        let mut server = self.hash(client_random, server_random);
        client.truncate(self.key_len());
        server.truncate(self.key_len());
        (client, server)
    }
}

/// SHA-256 fingerprint of an RA2 public key blob, as shown to users and pinned.
#[must_use]
pub fn fingerprint(blob: &[u8]) -> [u8; 32] {
    Sha256::digest(blob).into()
}

/// The wire form of a public key: `u32 bits`, modulus, exponent (both padded
/// to the modulus length).
fn key_blob(key: &PublicEncryptingKey) -> Result<Vec<u8>, AuthError> {
    let spki = key.as_der().map_err(crypto_error)?;
    let (modulus, exponent) =
        der::rsa_components(spki.as_ref()).ok_or_else(|| crypto_error("spki"))?;
    let len = modulus.len();
    let exponent = der::pad_to(&exponent, len).ok_or_else(|| crypto_error("exponent"))?;
    let mut blob = Vec::with_capacity(4 + 2 * len);
    blob.put_u32(u32::try_from(key.key_size_bits()).map_err(crypto_error)?);
    blob.extend_from_slice(&modulus);
    blob.extend_from_slice(&exponent);
    Ok(blob)
}

/// Reads a peer public key. Returns the key and its exact wire blob, or
/// `Ok(None)` if more input is needed.
fn read_key(r: &mut Reader<'_>) -> Result<Option<(PublicEncryptingKey, Vec<u8>)>, AuthError> {
    let start = r.consumed();
    let Ok(bits) = r.u32() else { return Ok(None) };
    if !(MIN_KEY_BITS..=MAX_KEY_BITS).contains(&bits) {
        return Err(AuthError::new(format!(
            "RSA key of {bits} bits is not acceptable"
        )));
    }
    let len = (bits as usize).div_ceil(8);
    let (Ok(modulus), Ok(exponent)) = (r.bytes(len), r.bytes(len)) else {
        return Ok(None);
    };
    let key = PublicEncryptingKey::from_der(&der::rsa_spki(modulus, exponent))
        .map_err(|_| AuthError::new("the peer's RSA key is invalid"))?;
    let mut blob = Vec::with_capacity(4 + 2 * len);
    blob.put_u32(bits);
    blob.extend_from_slice(modulus);
    blob.extend_from_slice(exponent);
    debug_assert_eq!(r.consumed() - start, blob.len());
    Ok(Some((key, blob)))
}

fn random(len: usize) -> Result<Zeroizing<Vec<u8>>, AuthError> {
    let mut bytes = Zeroizing::new(vec![0u8; len]);
    SystemRandom::new().fill(&mut bytes).map_err(crypto_error)?;
    Ok(bytes)
}

fn encrypt_to(
    key: &PublicEncryptingKey,
    plaintext: &[u8],
    output: &mut Vec<u8>,
) -> Result<(), AuthError> {
    let key = Pkcs1PublicEncryptingKey::new(key.clone()).map_err(crypto_error)?;
    let mut ciphertext = vec![0u8; key.ciphertext_size()];
    let written = key
        .encrypt(plaintext, &mut ciphertext)
        .map_err(crypto_error)?;
    output.put_u16(u16::try_from(written.len()).map_err(crypto_error)?);
    output.extend_from_slice(written);
    Ok(())
}

/// Reads a `u16`-prefixed RSA-encrypted random number and decrypts it.
fn read_random(
    r: &mut Reader<'_>,
    key: &PrivateDecryptingKey,
) -> Result<Option<Zeroizing<Vec<u8>>>, AuthError> {
    let Ok(len) = r.u16() else { return Ok(None) };
    if usize::from(len) != key.key_size_bytes() {
        return Err(AuthError::new(
            "encrypted random number has the wrong length",
        ));
    }
    let Ok(ciphertext) = r.bytes(usize::from(len)) else {
        return Ok(None);
    };
    let key = Pkcs1PrivateDecryptingKey::new(key.clone()).map_err(crypto_error)?;
    let mut plaintext = Zeroizing::new(vec![0u8; key.min_output_size()]);
    let len = key
        .decrypt(ciphertext, &mut plaintext)
        .map_err(|_| AuthError::new("could not decrypt the random number"))?
        .len();
    if len == 0 || len > 64 {
        return Err(AuthError::new("random number has an unexpected length"));
    }
    plaintext.truncate(len);
    Ok(Some(plaintext))
}

fn read_hash(r: &mut Reader<'_>, expected: &[u8]) -> Result<bool, AuthError> {
    let Ok(received) = r.bytes(expected.len()) else {
        return Ok(false);
    };
    if bool::from(received.ct_eq(expected)) {
        Ok(true)
    } else {
        Err(AuthError::new(
            "RSA-AES key confirmation failed: possible man-in-the-middle",
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientState {
    ServerKey,
    AwaitConfirmation,
    SendClientKey,
    ServerRandom,
    ServerHash,
    Subtype,
    Credentials { username: bool },
}

/// The viewer side of RA2 / RA2_256.
pub struct Ra2Client {
    variant: Ra2Variant,
    state: ClientState,
    server_key: Option<PublicEncryptingKey>,
    server_blob: Vec<u8>,
    client_key: Option<PrivateDecryptingKey>,
    client_blob: Vec<u8>,
    username: Option<SecretString>,
    password: Option<SecretString>,
}

impl Ra2Client {
    /// Creates the viewer side. The server key is always presented for
    /// confirmation ([`AuthRequest::ConfirmServerKey`]); the caller answers
    /// immediately when it matches a pinned fingerprint.
    #[must_use]
    pub fn new(
        variant: Ra2Variant,
        username: Option<SecretString>,
        password: Option<SecretString>,
    ) -> Self {
        Self {
            variant,
            state: ClientState::ServerKey,
            server_key: None,
            server_blob: Vec::new(),
            client_key: None,
            client_blob: Vec::new(),
            username,
            password,
        }
    }

    fn send_client_key(&mut self, output: &mut Vec<u8>) -> Result<(), AuthError> {
        let key = PrivateDecryptingKey::generate(KeySize::Rsa2048).map_err(crypto_error)?;
        self.client_blob = key_blob(&key.public_key())?;
        output.extend_from_slice(&self.client_blob);
        self.client_key = Some(key);
        Ok(())
    }
}

impl Authenticator for Ra2Client {
    fn advance(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError> {
        let mut used = 0;
        loop {
            let mut r = Reader::new(input.get(used..).unwrap_or_default());
            match self.state {
                ClientState::ServerKey => {
                    let Some((key, blob)) = read_key(&mut r)? else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    let fingerprint = fingerprint(&blob);
                    self.server_key = Some(key);
                    self.server_blob = blob;
                    self.state = ClientState::AwaitConfirmation;
                    let request = AuthRequest::ConfirmServerKey { fingerprint };
                    return Ok((used + r.consumed(), AuthProgress::Paused(request)));
                }
                ClientState::AwaitConfirmation => {
                    return Err(AuthError::new("server key not confirmed"));
                }
                ClientState::SendClientKey => {
                    self.send_client_key(output)?;
                    self.state = ClientState::ServerRandom;
                }
                ClientState::ServerRandom => {
                    let client_key = self
                        .client_key
                        .as_ref()
                        .ok_or_else(|| crypto_error("no client key"))?;
                    let Some(server_random) = read_random(&mut r, client_key)? else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    let client_random = random(self.variant.random_len())?;
                    let server_key = self
                        .server_key
                        .as_ref()
                        .ok_or_else(|| crypto_error("no server key"))?;
                    encrypt_to(server_key, &client_random, output)?;
                    let (send_key, receive_key) =
                        self.variant.session_keys(&server_random, &client_random);
                    self.state = ClientState::ServerHash;
                    let upgrade = TransportUpgrade::Ra2 {
                        send_key,
                        receive_key,
                    };
                    return Ok((used + r.consumed(), AuthProgress::UpgradeTransport(upgrade)));
                }
                ClientState::ServerHash => {
                    let expected = self.variant.hash(&self.server_blob, &self.client_blob);
                    if !read_hash(&mut r, &expected)? {
                        return Ok((used, AuthProgress::NeedMore));
                    }
                    output.extend_from_slice(
                        &self.variant.hash(&self.client_blob, &self.server_blob),
                    );
                    self.state = ClientState::Subtype;
                }
                ClientState::Subtype => {
                    let Ok(subtype) = r.u8() else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    let username = match subtype {
                        SUBTYPE_USER_PASSWORD => true,
                        SUBTYPE_PASSWORD => false,
                        _ => return Err(AuthError::new("server sent an invalid RSA-AES subtype")),
                    };
                    self.state = ClientState::Credentials { username };
                }
                ClientState::Credentials {
                    username: needs_username,
                } => {
                    let have_username = self.username.is_some() || !needs_username;
                    let Some(password) = self.password.as_ref().filter(|_| have_username) else {
                        let request = AuthRequest::Credentials {
                            username: needs_username,
                        };
                        return Ok((used, AuthProgress::Paused(request)));
                    };
                    let user_bytes = if needs_username {
                        self.username
                            .as_ref()
                            .map(|u| u.expose_secret().as_bytes())
                            .unwrap_or_default()
                    } else {
                        &[]
                    };
                    let pass = password.expose_secret().as_bytes();
                    let too_long =
                        || AuthError::new("username or password is longer than 255 bytes");
                    output.put_u8(u8::try_from(user_bytes.len()).map_err(|_| too_long())?);
                    output.extend_from_slice(user_bytes);
                    output.put_u8(u8::try_from(pass.len()).map_err(|_| too_long())?);
                    output.extend_from_slice(pass);
                    return Ok((used, AuthProgress::Complete));
                }
            }
            used += r.consumed();
        }
    }

    fn respond(&mut self, response: AuthResponse) -> Result<(), AuthError> {
        match (response, self.state) {
            (AuthResponse::ServerKeyAccepted, ClientState::AwaitConfirmation) => {
                // The client key goes out only once the server is trusted.
                self.state = ClientState::SendClientKey;
                Ok(())
            }
            (AuthResponse::Credentials { username, password }, ClientState::Credentials { .. }) => {
                self.username = username.or_else(|| self.username.take());
                self.password = Some(password);
                Ok(())
            }
            _ => Err(AuthError::new("unexpected RSA-AES response")),
        }
    }
}

/// What the agent checks in the RA2 credentials step.
pub enum Ra2Credentials {
    /// Username and password (subtype 1).
    UserPassword(Box<dyn PlainVerifier>),
    /// Password only (subtype 2).
    Password(SecretString),
}

/// The agent's long-term RSA key (its identity for RA2 viewers).
pub struct Ra2ServerKey {
    private: PrivateDecryptingKey,
    blob: Vec<u8>,
}

impl Ra2ServerKey {
    /// Generates a new 2048-bit key.
    ///
    /// # Errors
    /// [`AuthError`] if key generation fails.
    pub fn generate() -> Result<Self, AuthError> {
        Self::from_private(PrivateDecryptingKey::generate(KeySize::Rsa2048).map_err(crypto_error)?)
    }

    /// Loads a PKCS#8-encoded key.
    ///
    /// # Errors
    /// [`AuthError`] if the key is invalid.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, AuthError> {
        Self::from_private(PrivateDecryptingKey::from_pkcs8(pkcs8).map_err(crypto_error)?)
    }

    fn from_private(private: PrivateDecryptingKey) -> Result<Self, AuthError> {
        let blob = key_blob(&private.public_key())?;
        Ok(Self { private, blob })
    }

    /// The fingerprint viewers see and pin.
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 32] {
        fingerprint(&self.blob)
    }

    /// The private key as PKCS#8 DER, for storing (keep it 0600).
    ///
    /// # Errors
    /// [`AuthError`] if serialization fails.
    pub fn to_pkcs8(&self) -> Result<Zeroizing<Vec<u8>>, AuthError> {
        let der = self.private.as_der().map_err(crypto_error)?;
        Ok(Zeroizing::new(der.as_ref().to_vec()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerState {
    SendKey,
    ClientKey,
    ClientRandom,
    SendHash,
    ClientHash,
    Credentials,
}

/// The agent side of RA2 / RA2_256.
pub struct Ra2Server {
    variant: Ra2Variant,
    state: ServerState,
    key: std::sync::Arc<Ra2ServerKey>,
    credentials: Ra2Credentials,
    client_key: Option<PublicEncryptingKey>,
    client_blob: Vec<u8>,
    server_random: Zeroizing<Vec<u8>>,
}

impl Ra2Server {
    /// Creates the agent side.
    #[must_use]
    pub fn new(
        variant: Ra2Variant,
        key: std::sync::Arc<Ra2ServerKey>,
        credentials: Ra2Credentials,
    ) -> Self {
        Self {
            variant,
            state: ServerState::SendKey,
            key,
            credentials,
            client_key: None,
            client_blob: Vec::new(),
            server_random: Zeroizing::new(Vec::new()),
        }
    }

    fn check(&self, r: &mut Reader<'_>) -> Result<Option<()>, AuthError> {
        let Ok(user_len) = r.u8() else {
            return Ok(None);
        };
        let Ok(user) = r.bytes(usize::from(user_len)) else {
            return Ok(None);
        };
        let Ok(pass_len) = r.u8() else {
            return Ok(None);
        };
        let Ok(pass) = r.bytes(usize::from(pass_len)) else {
            return Ok(None);
        };
        let password = SecretString::from(String::from_utf8_lossy(pass).into_owned());
        let ok = match &self.credentials {
            Ra2Credentials::UserPassword(verifier) => {
                verifier.verify(&String::from_utf8_lossy(user), &password)
            }
            Ra2Credentials::Password(expected) => bool::from(
                expected
                    .expose_secret()
                    .as_bytes()
                    .ct_eq(password.expose_secret().as_bytes()),
            ),
        };
        if ok {
            Ok(Some(()))
        } else {
            Err(AuthError::new("authentication failed"))
        }
    }
}

impl Authenticator for Ra2Server {
    fn advance(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError> {
        let mut used = 0;
        loop {
            let mut r = Reader::new(input.get(used..).unwrap_or_default());
            match self.state {
                ServerState::SendKey => {
                    output.extend_from_slice(&self.key.blob);
                    self.state = ServerState::ClientKey;
                }
                ServerState::ClientKey => {
                    let Some((key, blob)) = read_key(&mut r)? else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    self.server_random = random(self.variant.random_len())?;
                    encrypt_to(&key, &self.server_random, output)?;
                    self.client_key = Some(key);
                    self.client_blob = blob;
                    self.state = ServerState::ClientRandom;
                }
                ServerState::ClientRandom => {
                    let Some(client_random) = read_random(&mut r, &self.key.private)? else {
                        return Ok((used, AuthProgress::NeedMore));
                    };
                    let (client_key, server_key) = self
                        .variant
                        .session_keys(&self.server_random, &client_random);
                    self.state = ServerState::SendHash;
                    let upgrade = TransportUpgrade::Ra2 {
                        send_key: server_key,
                        receive_key: client_key,
                    };
                    return Ok((used + r.consumed(), AuthProgress::UpgradeTransport(upgrade)));
                }
                ServerState::SendHash => {
                    output.extend_from_slice(&self.variant.hash(&self.key.blob, &self.client_blob));
                    self.state = ServerState::ClientHash;
                }
                ServerState::ClientHash => {
                    let expected = self.variant.hash(&self.client_blob, &self.key.blob);
                    if !read_hash(&mut r, &expected)? {
                        return Ok((used, AuthProgress::NeedMore));
                    }
                    output.put_u8(match self.credentials {
                        Ra2Credentials::UserPassword(_) => SUBTYPE_USER_PASSWORD,
                        Ra2Credentials::Password(_) => SUBTYPE_PASSWORD,
                    });
                    self.state = ServerState::Credentials;
                }
                ServerState::Credentials => {
                    return match self.check(&mut r)? {
                        Some(()) => Ok((used + r.consumed(), AuthProgress::Complete)),
                        None => Ok((used, AuthProgress::NeedMore)),
                    };
                }
            }
            used += r.consumed();
        }
    }
}

enum EaxKey {
    Aes128(Box<Eax<Aes128>>),
    Aes256(Box<Eax<Aes256>>),
}

impl EaxKey {
    fn new(key: &[u8]) -> Result<Self, AuthError> {
        match key.len() {
            16 => Ok(Self::Aes128(Box::new(Eax::new(GenericArray::from_slice(
                key,
            ))))),
            32 => Ok(Self::Aes256(Box::new(Eax::new(GenericArray::from_slice(
                key,
            ))))),
            _ => Err(AuthError::new("RA2 session key has an invalid length")),
        }
    }

    fn seal(
        &self,
        nonce: &[u8; 16],
        aad: &[u8],
        buffer: &mut [u8],
    ) -> Result<[u8; TAG_LEN], AuthError> {
        let nonce = GenericArray::from_slice(nonce);
        let tag = match self {
            Self::Aes128(c) => c.encrypt_in_place_detached(nonce, aad, buffer),
            Self::Aes256(c) => c.encrypt_in_place_detached(nonce, aad, buffer),
        }
        .map_err(crypto_error)?;
        Ok(tag.into())
    }

    fn open(
        &self,
        nonce: &[u8; 16],
        aad: &[u8],
        buffer: &mut [u8],
        tag: &[u8],
    ) -> Result<(), AuthError> {
        let nonce = GenericArray::from_slice(nonce);
        let tag = GenericArray::from_slice(tag);
        match self {
            Self::Aes128(c) => c.decrypt_in_place_detached(nonce, aad, buffer, tag),
            Self::Aes256(c) => c.decrypt_in_place_detached(nonce, aad, buffer, tag),
        }
        .map_err(|_| {
            AuthError::new("RA2 frame failed authentication: the stream was tampered with")
        })
    }
}

/// AES-EAX framing for an RA2 session: `u16 length`, ciphertext, 16-byte
/// MAC; the length is the associated data and a little-endian 128-bit
/// message counter is the nonce.
pub struct Ra2Cipher {
    sender: EaxKey,
    receiver: EaxKey,
    sent: u128,
    received: u128,
}

impl Ra2Cipher {
    /// Creates the framing from the keys in [`TransportUpgrade::Ra2`].
    ///
    /// # Errors
    /// [`AuthError`] if a key has an invalid length.
    pub fn new(send_key: &[u8], receive_key: &[u8]) -> Result<Self, AuthError> {
        Ok(Self {
            sender: EaxKey::new(send_key)?,
            receiver: EaxKey::new(receive_key)?,
            sent: 0,
            received: 0,
        })
    }

    /// Encrypts `plaintext` into one or more frames appended to `output`.
    ///
    /// # Errors
    /// [`AuthError`] if encryption fails.
    pub fn seal(&mut self, plaintext: &[u8], output: &mut Vec<u8>) -> Result<(), AuthError> {
        for chunk in plaintext.chunks(MAX_FRAME) {
            let header = u16::try_from(chunk.len())
                .map_err(crypto_error)?
                .to_be_bytes();
            let mut buffer = chunk.to_vec();
            let tag = self
                .sender
                .seal(&self.sent.to_le_bytes(), &header, &mut buffer)?;
            self.sent = self.sent.wrapping_add(1);
            output.extend_from_slice(&header);
            output.extend_from_slice(&buffer);
            output.extend_from_slice(&tag);
        }
        Ok(())
    }

    /// Decrypts the frame at the start of `input`, returning the plaintext and
    /// the bytes consumed, or `None` if the frame is incomplete.
    ///
    /// # Errors
    /// [`AuthError`] if the frame fails authentication.
    pub fn open(&mut self, input: &[u8]) -> Result<Option<(Vec<u8>, usize)>, AuthError> {
        let mut r = Reader::new(input);
        let Ok(len) = r.u16() else { return Ok(None) };
        let (Ok(ciphertext), Ok(tag)) = (r.bytes(usize::from(len)), r.bytes(TAG_LEN)) else {
            return Ok(None);
        };
        let mut buffer = ciphertext.to_vec();
        self.receiver.open(
            &self.received.to_le_bytes(),
            &len.to_be_bytes(),
            &mut buffer,
            tag,
        )?;
        self.received = self.received.wrapping_add(1);
        Ok(Some((buffer, r.consumed())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Bob;
    impl PlainVerifier for Bob {
        fn verify(&self, username: &str, password: &SecretString) -> bool {
            username == "bob" && password.expose_secret() == "builder"
        }
    }

    fn secret(s: &str) -> SecretString {
        SecretString::from(s.to_owned())
    }

    /// Bytes in flight between the two sides, plaintext before the upgrade
    /// and AES-EAX framed after it.
    #[derive(Default)]
    struct Pipe {
        to_client: Vec<u8>,
        to_server: Vec<u8>,
        client_cipher: Option<Ra2Cipher>,
        server_cipher: Option<Ra2Cipher>,
    }

    fn deliver(cipher: Option<&mut Ra2Cipher>, raw: &mut Vec<u8>) -> Vec<u8> {
        let Some(cipher) = cipher else {
            return std::mem::take(raw);
        };
        let mut plain = Vec::new();
        while let Some((bytes, used)) = cipher.open(raw).unwrap() {
            plain.extend(bytes);
            raw.drain(..used);
        }
        plain
    }

    fn send(cipher: Option<&mut Ra2Cipher>, plain: Vec<u8>, raw: &mut Vec<u8>) {
        match cipher {
            Some(cipher) => cipher.seal(&plain, raw).unwrap(),
            None => raw.extend(plain),
        }
    }

    fn handshake(
        variant: Ra2Variant,
        credentials: Ra2Credentials,
        user: Option<&str>,
        pass: &str,
    ) -> Result<[u8; 32], AuthError> {
        let key = Arc::new(Ra2ServerKey::generate()?);
        let mut server = Ra2Server::new(variant, key, credentials);
        let mut client = Ra2Client::new(variant, None, None);
        let mut pipe = Pipe::default();
        let (mut client_in, mut server_in) = (Vec::new(), Vec::new());
        let (mut client_done, mut server_done) = (false, false);
        let mut seen = None;
        for _ in 0..40 {
            if !server_done {
                server_in.extend(deliver(pipe.server_cipher.as_mut(), &mut pipe.to_server));
                let mut out = Vec::new();
                let (n, progress) = server.advance(&server_in, &mut out)?;
                server_in.drain(..n);
                send(pipe.server_cipher.as_mut(), out, &mut pipe.to_client);
                match progress {
                    AuthProgress::UpgradeTransport(TransportUpgrade::Ra2 {
                        send_key,
                        receive_key,
                    }) => {
                        pipe.server_cipher = Some(Ra2Cipher::new(&send_key, &receive_key)?);
                    }
                    AuthProgress::Complete => server_done = true,
                    _ => {}
                }
            }
            if !client_done {
                client_in.extend(deliver(pipe.client_cipher.as_mut(), &mut pipe.to_client));
                let mut out = Vec::new();
                let (n, progress) = client.advance(&client_in, &mut out)?;
                client_in.drain(..n);
                send(pipe.client_cipher.as_mut(), out, &mut pipe.to_server);
                match progress {
                    AuthProgress::Paused(AuthRequest::ConfirmServerKey { fingerprint }) => {
                        seen = Some(fingerprint);
                        client.respond(AuthResponse::ServerKeyAccepted)?;
                    }
                    AuthProgress::Paused(AuthRequest::Credentials { username }) => {
                        assert_eq!(username, user.is_some());
                        client.respond(AuthResponse::Credentials {
                            username: user.map(secret),
                            password: secret(pass),
                        })?;
                    }
                    AuthProgress::UpgradeTransport(TransportUpgrade::Ra2 {
                        send_key,
                        receive_key,
                    }) => {
                        pipe.client_cipher = Some(Ra2Cipher::new(&send_key, &receive_key)?);
                    }
                    AuthProgress::Complete => client_done = true,
                    _ => {}
                }
            }
            if client_done && server_done {
                return seen.ok_or_else(|| AuthError::new("no key confirmation"));
            }
        }
        Err(AuthError::new("did not finish"))
    }

    #[test]
    fn ra2_user_password() {
        handshake(
            Ra2Variant::Sha1,
            Ra2Credentials::UserPassword(Box::new(Bob)),
            Some("bob"),
            "builder",
        )
        .unwrap();
    }

    #[test]
    fn ra2_256_password_only() {
        handshake(
            Ra2Variant::Sha256,
            Ra2Credentials::Password(secret("pw")),
            None,
            "pw",
        )
        .unwrap();
    }

    #[test]
    fn wrong_password_is_rejected() {
        let err = handshake(
            Ra2Variant::Sha256,
            Ra2Credentials::Password(secret("pw")),
            None,
            "nope",
        )
        .unwrap_err();
        assert_eq!(err.reason, "authentication failed");
    }

    #[test]
    fn cipher_detects_tampering_and_replay() {
        let mut alice = Ra2Cipher::new(&[1; 16], &[2; 16]).unwrap();
        let mut bob = Ra2Cipher::new(&[2; 16], &[1; 16]).unwrap();
        let mut wire = Vec::new();
        alice.seal(b"hello", &mut wire).unwrap();
        let first = wire.clone();
        assert_eq!(
            bob.open(&wire).unwrap(),
            Some((b"hello".to_vec(), wire.len()))
        );
        // Replaying the same frame fails: the nonce counter moved on.
        assert!(bob.open(&first).is_err());
        let mut wire = Vec::new();
        alice.seal(b"world", &mut wire).unwrap();
        if let Some(byte) = wire.get_mut(3) {
            *byte ^= 1;
        }
        assert!(bob.open(&wire).is_err());
    }

    #[test]
    fn cipher_waits_for_complete_frames_and_splits_large_input() {
        let mut alice = Ra2Cipher::new(&[3; 32], &[4; 32]).unwrap();
        let mut bob = Ra2Cipher::new(&[4; 32], &[3; 32]).unwrap();
        let message = vec![7u8; MAX_FRAME * 2 + 5];
        let mut wire = Vec::new();
        alice.seal(&message, &mut wire).unwrap();
        assert_eq!(bob.open(wire.get(..10).unwrap()).unwrap(), None);
        let mut received = Vec::new();
        let mut rest = wire.as_slice();
        while let Some((bytes, used)) = bob.open(rest).unwrap() {
            received.extend(bytes);
            rest = rest.get(used..).unwrap();
        }
        assert_eq!(received, message);
    }

    #[test]
    fn small_keys_are_rejected() {
        let mut client = Ra2Client::new(Ra2Variant::Sha1, None, None);
        let mut input = Vec::new();
        input.put_u32(1024);
        input.extend(vec![0xff; 256]);
        assert!(client.advance(&input, &mut Vec::new()).is_err());
    }

    #[test]
    fn pkcs8_round_trip_keeps_the_identity() {
        let key = Ra2ServerKey::generate().unwrap();
        let again = Ra2ServerKey::from_pkcs8(&key.to_pkcs8().unwrap()).unwrap();
        assert_eq!(key.fingerprint(), again.fingerprint());
    }

    #[test]
    fn fingerprint_matches_the_server_key() {
        let key = Ra2ServerKey::generate().unwrap();
        let mut client = Ra2Client::new(Ra2Variant::Sha1, None, None);
        let (_, progress) = client.advance(&key.blob, &mut Vec::new()).unwrap();
        assert_eq!(
            progress,
            AuthProgress::Paused(AuthRequest::ConfirmServerKey {
                fingerprint: key.fingerprint()
            })
        );
    }
}
