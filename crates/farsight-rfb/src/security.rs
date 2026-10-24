//! Security-type identifiers (RFC 6143 §7.1.2 and the IANA RFB registry).

/// A security type offered during the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityType {
    /// No authentication. Legacy: gated by policy (ADR-0007).
    None,
    /// DES challenge-response. Legacy: gated by policy (ADR-0007).
    VncAuth,
    /// RSA-AES with SHA-1 (`RA2`).
    Ra2,
    /// RSA-AES without session encryption (`RA2ne`).
    Ra2ne,
    /// TightVNC security type.
    Tight,
    /// VeNCrypt, with a further [`VeNCryptSubtype`] negotiation.
    VeNCrypt,
    /// RSA-AES-256 with SHA-256 (`RA2_256`).
    Ra256,
    /// RSA-AES-256 without session encryption (`RA2ne_256`).
    Rane256,
    /// Any type farsight does not recognise.
    Unknown(u8),
}

impl SecurityType {
    /// Maps a wire code to a security type.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::None,
            2 => Self::VncAuth,
            5 => Self::Ra2,
            6 => Self::Ra2ne,
            16 => Self::Tight,
            19 => Self::VeNCrypt,
            129 => Self::Ra256,
            130 => Self::Rane256,
            other => Self::Unknown(other),
        }
    }

    /// The wire code for this security type.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::None => 1,
            Self::VncAuth => 2,
            Self::Ra2 => 5,
            Self::Ra2ne => 6,
            Self::Tight => 16,
            Self::VeNCrypt => 19,
            Self::Ra256 => 129,
            Self::Rane256 => 130,
            Self::Unknown(code) => code,
        }
    }

    /// Whether this type offers no protection against eavesdropping or brute
    /// force and must therefore be explicitly enabled off-loopback.
    #[must_use]
    pub const fn is_legacy_insecure(self) -> bool {
        matches!(self, Self::None | Self::VncAuth)
    }
}

/// A VeNCrypt subtype, negotiated after [`SecurityType::VeNCrypt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VeNCryptSubtype {
    /// Username/password in plaintext. Never used without TLS.
    Plain,
    /// Anonymous TLS, no auth. Unsupported (ADR-0007).
    TlsNone,
    /// Anonymous TLS with VncAuth. Unsupported (ADR-0007).
    TlsVnc,
    /// Anonymous TLS with Plain. Unsupported (ADR-0007).
    TlsPlain,
    /// X.509-authenticated TLS, no further auth.
    X509None,
    /// X.509-authenticated TLS with VncAuth.
    X509Vnc,
    /// X.509-authenticated TLS with Plain.
    X509Plain,
    /// Any subtype farsight does not recognise.
    Unknown(u32),
}

impl VeNCryptSubtype {
    /// Maps a wire code to a subtype.
    #[must_use]
    pub const fn from_code(code: u32) -> Self {
        match code {
            256 => Self::Plain,
            257 => Self::TlsNone,
            258 => Self::TlsVnc,
            259 => Self::TlsPlain,
            260 => Self::X509None,
            261 => Self::X509Vnc,
            262 => Self::X509Plain,
            other => Self::Unknown(other),
        }
    }

    /// The wire code for this subtype.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Plain => 256,
            Self::TlsNone => 257,
            Self::TlsVnc => 258,
            Self::TlsPlain => 259,
            Self::X509None => 260,
            Self::X509Vnc => 261,
            Self::X509Plain => 262,
            Self::Unknown(code) => code,
        }
    }

    /// Whether the subtype runs over TLS with an authenticated server, the only
    /// VeNCrypt subtypes farsight supports.
    #[must_use]
    pub const fn authenticates_server(self) -> bool {
        matches!(self, Self::X509None | Self::X509Vnc | Self::X509Plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn security_type_codes_round_trip(code in any::<u8>()) {
            prop_assert_eq!(SecurityType::from_code(code).code(), code);
        }

        #[test]
        fn vencrypt_subtype_codes_round_trip(code in any::<u32>()) {
            prop_assert_eq!(VeNCryptSubtype::from_code(code).code(), code);
        }
    }

    #[test]
    fn only_none_and_vncauth_are_legacy() {
        assert!(SecurityType::None.is_legacy_insecure());
        assert!(SecurityType::VncAuth.is_legacy_insecure());
        assert!(!SecurityType::VeNCrypt.is_legacy_insecure());
        assert!(!SecurityType::Ra256.is_legacy_insecure());
    }

    #[test]
    fn anonymous_tls_subtypes_do_not_authenticate_the_server() {
        for subtype in [
            VeNCryptSubtype::TlsNone,
            VeNCryptSubtype::TlsVnc,
            VeNCryptSubtype::TlsPlain,
            VeNCryptSubtype::Plain,
        ] {
            assert!(!subtype.authenticates_server());
        }
        assert!(VeNCryptSubtype::X509Vnc.authenticates_server());
    }
}
