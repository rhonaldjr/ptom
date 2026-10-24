//! VNC Authentication (RFC 6143 §7.2.2): DES challenge-response.
//!
//! **Legacy and weak.** Only the first 8 password bytes count, DES is broken,
//! and the exchange can be cracked offline. Farsight only allows it on
//! loopback or with explicit opt-in (ADR-0007).
//!
//! The DES key is the password with each byte's bits reversed: a quirk of
//! the original implementation that every VNC product reproduces.

use des::Des;
use des::cipher::generic_array::GenericArray;
use des::cipher::{BlockEncrypt, KeyInit};
use farsight_rfb::auth::{AuthError, AuthProgress, Authenticator};
use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// Length of the challenge and the response.
pub const CHALLENGE_LEN: usize = 16;

/// Computes the response to `challenge` for `password`.
#[must_use]
pub fn response(password: &[u8], challenge: &[u8; CHALLENGE_LEN]) -> [u8; CHALLENGE_LEN] {
    let mut key = Zeroizing::new([0u8; 8]);
    for (k, p) in key.iter_mut().zip(password) {
        *k = p.reverse_bits();
    }
    let cipher = Des::new(GenericArray::from_slice(key.as_ref()));
    let mut out = *challenge;
    for block in out.as_chunks_mut::<8>().0 {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    out
}

/// The viewer side: answers the server's challenge.
pub struct VncAuthClient {
    password: SecretString,
}

impl VncAuthClient {
    /// Creates the client side. Passwords longer than 8 bytes are truncated,
    /// as in every VNC implementation.
    #[must_use]
    pub fn new(password: SecretString) -> Self {
        Self { password }
    }
}

impl Authenticator for VncAuthClient {
    fn advance(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError> {
        let Some(challenge) = input.first_chunk::<CHALLENGE_LEN>() else {
            return Ok((0, AuthProgress::NeedMore));
        };
        output.extend_from_slice(&response(
            self.password.expose_secret().as_bytes(),
            challenge,
        ));
        Ok((CHALLENGE_LEN, AuthProgress::Complete))
    }
}

/// The agent side: sends a challenge and checks the response.
pub struct VncAuthServer {
    password: SecretString,
    challenge: [u8; CHALLENGE_LEN],
    sent: bool,
}

impl VncAuthServer {
    /// Creates the server side. `challenge` **must** come from a
    /// cryptographically secure random source, fresh for every connection.
    #[must_use]
    pub fn new(password: SecretString, challenge: [u8; CHALLENGE_LEN]) -> Self {
        Self {
            password,
            challenge,
            sent: false,
        }
    }
}

impl Authenticator for VncAuthServer {
    fn advance(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError> {
        if !self.sent {
            output.extend_from_slice(&self.challenge);
            self.sent = true;
        }
        let Some(received) = input.first_chunk::<CHALLENGE_LEN>() else {
            return Ok((0, AuthProgress::NeedMore));
        };
        let expected = Zeroizing::new(response(
            self.password.expose_secret().as_bytes(),
            &self.challenge,
        ));
        if bool::from(expected.ct_eq(received)) {
            Ok((CHALLENGE_LEN, AuthProgress::Complete))
        } else {
            Err(AuthError::new("authentication failed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(s: &str) -> SecretString {
        SecretString::from(s.to_owned())
    }

    #[test]
    fn known_answer_matches_openssl_des_with_bit_reversed_key() {
        // openssl enc -des-ecb -K 0E86CECEEEF64E26 (bit-reversed "password").
        let expected = [
            0x56, 0x45, 0xab, 0xeb, 0x5f, 0x1e, 0x64, 0x75, 0xe8, 0xfe, 0xb1, 0x1b, 0xeb, 0x66,
            0xea, 0x19,
        ];
        assert_eq!(response(b"password", b"0123456789abcdef"), expected);
        // Only the first 8 bytes of the password matter.
        assert_eq!(
            response(b"password-and-more", b"0123456789abcdef"),
            expected
        );
    }

    #[test]
    fn client_and_server_agree_on_the_right_password() {
        let challenge = *b"0123456789abcdef";
        let mut server = VncAuthServer::new(secret("hunter2"), challenge);
        let mut client = VncAuthClient::new(secret("hunter2"));
        let mut to_client = Vec::new();
        assert_eq!(
            server.advance(&[], &mut to_client),
            Ok((0, AuthProgress::NeedMore))
        );
        assert_eq!(to_client, challenge);
        let mut to_server = Vec::new();
        assert_eq!(
            client.advance(&to_client, &mut to_server),
            Ok((16, AuthProgress::Complete))
        );
        assert_eq!(
            server.advance(&to_server, &mut Vec::new()),
            Ok((16, AuthProgress::Complete))
        );
    }

    #[test]
    fn wrong_password_is_rejected() {
        let challenge = [7u8; 16];
        let mut server = VncAuthServer::new(secret("right"), challenge);
        server.advance(&[], &mut Vec::new()).unwrap();
        let wrong = response(b"wrong", &challenge);
        assert!(server.advance(&wrong, &mut Vec::new()).is_err());
    }

    #[test]
    fn client_waits_for_a_full_challenge() {
        let mut client = VncAuthClient::new(secret("x"));
        assert_eq!(
            client.advance(&[0; 15], &mut Vec::new()),
            Ok((0, AuthProgress::NeedMore))
        );
    }
}
