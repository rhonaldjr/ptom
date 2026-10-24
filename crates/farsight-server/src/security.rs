//! Which security types the agent offers, and the authenticators behind
//! them. Also remembers which username a verifier accepted, for the owner
//! check (U-2).

use std::sync::{Arc, OnceLock};

use farsight_rfb::auth::{Authenticator, NoAuth};
use farsight_rfb::{ProtocolError, SecurityType};
use farsight_security::SecurityPolicy;
use farsight_security::ra2::{Ra2Credentials, Ra2Server, Ra2Variant};
use farsight_security::vencrypt::{PlainVerifier, VeNCryptServer};
use farsight_security::vnc_auth::{CHALLENGE_LEN, VncAuthServer};
use secrecy::SecretString;

use crate::ServerSessionConfig;
use crate::ports::UserVerifier;

/// Wraps the agent's verifier and records the username it accepted.
struct RecordingVerifier {
    inner: Arc<dyn UserVerifier>,
    verified: Arc<OnceLock<String>>,
}

impl PlainVerifier for RecordingVerifier {
    fn verify(&self, username: &str, password: &SecretString) -> bool {
        let ok = self.inner.verify(username, password);
        if ok {
            // Only one login happens per connection; a second set is a no-op.
            let _ = self.verified.set(username.to_owned());
        }
        ok
    }
}

/// The types to offer: the configured ones the network policy permits and
/// for which the needed key, verifier or password is present.
pub(crate) fn offer(config: &ServerSessionConfig) -> Vec<SecurityType> {
    let policy = SecurityPolicy {
        allow_legacy: config.loopback || config.allow_legacy,
    };
    let has_credentials = config.user_verifier.is_some() || config.vnc_password.is_some();
    policy
        .server_offer(&config.security_types)
        .into_iter()
        .filter(|t| match t {
            SecurityType::None => true,
            SecurityType::VncAuth => config.vnc_password.is_some(),
            SecurityType::VeNCrypt => config.tls_available && has_credentials,
            SecurityType::Ra2 | SecurityType::Ra256 => config.ra2_key.is_some() && has_credentials,
            _ => false,
        })
        .fold(Vec::new(), |mut out, t| {
            if !out.contains(&t) {
                out.push(t);
            }
            out
        })
}

fn challenge(config: &ServerSessionConfig) -> [u8; CHALLENGE_LEN] {
    let mut challenge = [0u8; CHALLENGE_LEN];
    config.entropy.fill(&mut challenge);
    challenge
}

/// Builds the authenticator for `security`, which must be in [`offer`].
pub(crate) fn authenticator(
    config: &ServerSessionConfig,
    security: SecurityType,
    verified: &Arc<OnceLock<String>>,
) -> Result<Box<dyn Authenticator>, ProtocolError> {
    let recording_verifier = || -> Option<Box<dyn PlainVerifier>> {
        config.user_verifier.as_ref().map(|inner| {
            Box::new(RecordingVerifier {
                inner: Arc::clone(inner),
                verified: Arc::clone(verified),
            }) as Box<dyn PlainVerifier>
        })
    };
    let missing = ProtocolError::InvalidState("security type selected without its material");
    Ok(match security {
        SecurityType::None => Box::new(NoAuth),
        SecurityType::VncAuth => {
            let password = config.vnc_password.clone().ok_or(missing)?;
            Box::new(VncAuthServer::new(password, challenge(config)))
        }
        SecurityType::VeNCrypt => Box::new(VeNCryptServer::new(
            recording_verifier(),
            config.vnc_password.clone(),
            challenge(config),
            false,
        )),
        SecurityType::Ra2 | SecurityType::Ra256 => {
            let variant = Ra2Variant::for_security_type(security).ok_or(missing.clone())?;
            let key = config.ra2_key.clone().ok_or(missing.clone())?;
            let credentials = match (recording_verifier(), config.vnc_password.clone()) {
                (Some(checker), _) => Ra2Credentials::UserPassword(checker),
                (None, Some(password)) => Ra2Credentials::Password(password),
                (None, None) => return Err(missing),
            };
            Box::new(Ra2Server::new(variant, key, credentials))
        }
        _ => return Err(missing),
    })
}
