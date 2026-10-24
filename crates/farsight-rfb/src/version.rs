//! The `ProtocolVersion` handshake message (RFC 6143 §7.1.1).

use crate::{DecodeError, wire};

/// Length in bytes of a `ProtocolVersion` message: `"RFB xxx.yyy\n"`.
pub const VERSION_MESSAGE_LEN: usize = 12;

/// An RFB protocol version that farsight speaks.
///
/// Variants are ordered oldest to newest, so `min` picks the older version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProtocolVersion {
    /// RFB 3.3: the server dictates the security type.
    V3_3,
    /// RFB 3.7: the client chooses from a list of security types.
    V3_7,
    /// RFB 3.8: adds a failure reason to `SecurityResult`.
    V3_8,
}

impl ProtocolVersion {
    /// The newest version farsight supports.
    pub const LATEST: Self = Self::V3_8;

    /// Encodes the 12-byte `ProtocolVersion` message.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; VERSION_MESSAGE_LEN] {
        match self {
            Self::V3_3 => *b"RFB 003.003\n",
            Self::V3_7 => *b"RFB 003.007\n",
            Self::V3_8 => *b"RFB 003.008\n",
        }
    }

    /// Decodes a `ProtocolVersion` message sent by a peer.
    ///
    /// Per RFC 6143 §7.1.1, non-standard 3.x minors are mapped onto the versions
    /// whose handshake they implement: below 3.7 (e.g. Apple's 3.5) is 3.3, and
    /// above 3.8 (e.g. Apple's 3.889) is 3.8.
    ///
    /// # Errors
    /// Returns [`DecodeError`] if the message is truncated, malformed, or its
    /// major version is not 3.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let &[b'R', b'F', b'B', b' ', a0, a1, a2, b'.', i0, i1, i2, b'\n'] =
            wire::take::<VERSION_MESSAGE_LEN>(bytes)?
        else {
            return Err(DecodeError::MalformedVersion);
        };
        let major = parse_three_digits([a0, a1, a2])?;
        let minor = parse_three_digits([i0, i1, i2])?;
        match (major, minor) {
            (3, 0..=6) => Ok(Self::V3_3),
            (3, 7) => Ok(Self::V3_7),
            (3, _) => Ok(Self::V3_8),
            _ => Err(DecodeError::UnsupportedVersion { major, minor }),
        }
    }

    /// The version a client replies with: the older of what the server offers
    /// and the newest the client supports.
    #[must_use]
    pub fn negotiate(server_offer: Self, client_max: Self) -> Self {
        server_offer.min(client_max)
    }
}

fn parse_three_digits(digits: [u8; 3]) -> Result<u16, DecodeError> {
    digits.iter().try_fold(0u16, |value, &digit| {
        if digit.is_ascii_digit() {
            Ok(value * 10 + u16::from(digit - b'0'))
        } else {
            Err(DecodeError::MalformedVersion)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn standard_versions_round_trip() {
        for version in [
            ProtocolVersion::V3_3,
            ProtocolVersion::V3_7,
            ProtocolVersion::V3_8,
        ] {
            assert_eq!(ProtocolVersion::decode(&version.to_bytes()), Ok(version));
        }
    }

    #[test]
    fn apple_variants_map_to_implemented_handshakes() {
        assert_eq!(
            ProtocolVersion::decode(b"RFB 003.005\n"),
            Ok(ProtocolVersion::V3_3)
        );
        assert_eq!(
            ProtocolVersion::decode(b"RFB 003.889\n"),
            Ok(ProtocolVersion::V3_8)
        );
    }

    #[test]
    fn rejects_other_major_versions() {
        assert_eq!(
            ProtocolVersion::decode(b"RFB 004.001\n"),
            Err(DecodeError::UnsupportedVersion { major: 4, minor: 1 })
        );
    }

    #[test]
    fn rejects_malformed_and_truncated_messages() {
        assert_eq!(
            ProtocolVersion::decode(b"SSH-2.0-xyz\n"),
            Err(DecodeError::MalformedVersion)
        );
        assert_eq!(
            ProtocolVersion::decode(b"RFB 003.00x\n"),
            Err(DecodeError::MalformedVersion)
        );
        assert_eq!(
            ProtocolVersion::decode(b"RFB 003"),
            Err(DecodeError::Truncated { needed: 12, got: 7 })
        );
    }

    #[test]
    fn negotiation_picks_the_older_version() {
        use ProtocolVersion::{V3_3, V3_8};
        assert_eq!(ProtocolVersion::negotiate(V3_8, V3_3), V3_3);
        assert_eq!(ProtocolVersion::negotiate(V3_3, V3_8), V3_3);
        assert_eq!(ProtocolVersion::negotiate(V3_8, V3_8), V3_8);
    }

    proptest! {
        #[test]
        fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..32)) {
            let _ = ProtocolVersion::decode(&bytes);
        }

        #[test]
        fn every_3x_minor_is_accepted(minor in 0u16..1000) {
            let message = format!("RFB 003.{minor:03}\n");
            prop_assert!(ProtocolVersion::decode(message.as_bytes()).is_ok());
        }
    }
}
