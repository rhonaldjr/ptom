//! The DNS-SD TXT record of an `_rfb._tcp` instance (ADR-0010).
//!
//! Farsight agents publish exactly three keys: `farsight=1`, `v=<rfb
//! version>` and `fp=<hex SHA-256 fingerprint>`. No usernames, OS or desktop
//! names. Other vendors' records are parsed leniently: unknown keys are
//! ignored, and malformed values are dropped rather than trusted.

/// Key marking a Farsight agent.
pub const KEY_FARSIGHT: &str = "farsight";
/// Key carrying the RFB protocol version, for example `3.8`.
pub const KEY_VERSION: &str = "v";
/// Key carrying the server's identity fingerprint as lowercase hex SHA-256.
pub const KEY_FINGERPRINT: &str = "fp";

/// The RFB version Farsight agents advertise.
pub const ADVERTISED_VERSION: &str = "3.8";

/// Length of a hex SHA-256 fingerprint.
pub const FINGERPRINT_HEX_LEN: usize = 64;

/// Longest accepted `v=` value.
const MAX_VERSION_LEN: usize = 8;

/// What a TXT record says about a server. Every field is a peer claim and is
/// unverified: a fingerprint here is a hint for TOFU, never proof.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TxtInfo {
    /// The record contains `farsight=1`.
    pub farsight: bool,
    /// The advertised RFB version (`major.minor`, digits only).
    pub version: Option<String>,
    /// The advertised fingerprint, normalised to lowercase hex.
    pub fingerprint: Option<String>,
}

/// Parses TXT key/value pairs. Keys compare case-insensitively (RFC 6763
/// §6.4); the first occurrence of a key wins (§6.4 too). A key with no value
/// is passed as `None`.
#[must_use]
pub fn parse_txt<'a>(pairs: impl IntoIterator<Item = (&'a str, Option<&'a [u8]>)>) -> TxtInfo {
    let mut info = TxtInfo::default();
    let mut seen = [false; 3];
    for (key, value) in pairs {
        let slot = if key.eq_ignore_ascii_case(KEY_FARSIGHT) {
            0
        } else if key.eq_ignore_ascii_case(KEY_VERSION) {
            1
        } else if key.eq_ignore_ascii_case(KEY_FINGERPRINT) {
            2
        } else {
            continue;
        };
        if let Some(first) = seen.get_mut(slot) {
            if *first {
                continue;
            }
            *first = true;
        }
        let value = value.and_then(|v| std::str::from_utf8(v).ok());
        match slot {
            0 => info.farsight = value == Some("1"),
            1 => info.version = value.and_then(parse_version),
            _ => info.fingerprint = value.and_then(parse_fingerprint),
        }
    }
    info
}

/// Builds the TXT pairs a Farsight agent advertises.
///
/// # Errors
/// [`InvalidFingerprint`] unless `fingerprint` is 64 hex digits.
pub fn build_txt(fingerprint: &str) -> Result<Vec<(String, String)>, InvalidFingerprint> {
    let fingerprint = parse_fingerprint(fingerprint).ok_or(InvalidFingerprint)?;
    Ok(vec![
        (KEY_FARSIGHT.to_owned(), "1".to_owned()),
        (KEY_VERSION.to_owned(), ADVERTISED_VERSION.to_owned()),
        (KEY_FINGERPRINT.to_owned(), fingerprint),
    ])
}

/// The fingerprint was not 64 hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("fingerprint must be 64 hex digits (SHA-256)")]
pub struct InvalidFingerprint;

/// Accepts exactly 64 hex digits and returns them in lowercase.
#[must_use]
pub fn parse_fingerprint(value: &str) -> Option<String> {
    (value.len() == FINGERPRINT_HEX_LEN && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn parse_version(value: &str) -> Option<String> {
    let (major, minor) = value.split_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (value.len() <= MAX_VERSION_LEN && digits(major) && digits(minor)).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const FP: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn pairs(
        list: &[(&'static str, Option<&'static str>)],
    ) -> Vec<(&'static str, Option<&'static [u8]>)> {
        list.iter()
            .map(|(k, v)| (*k, v.map(str::as_bytes)))
            .collect()
    }

    #[test]
    fn build_then_parse_round_trips() {
        let built = build_txt(&FP.to_ascii_uppercase()).unwrap();
        assert_eq!(built.len(), 3);
        let info = parse_txt(built.iter().map(|(k, v)| (k.as_str(), Some(v.as_bytes()))));
        assert_eq!(
            info,
            TxtInfo {
                farsight: true,
                version: Some("3.8".into()),
                fingerprint: Some(FP.into()),
            }
        );
    }

    #[test]
    fn build_only_has_minimal_keys() {
        let keys: Vec<_> = build_txt(FP).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["farsight", "v", "fp"]);
    }

    #[test]
    fn build_rejects_bad_fingerprints() {
        assert_eq!(build_txt("abc"), Err(InvalidFingerprint));
        assert_eq!(build_txt(&"g".repeat(64)), Err(InvalidFingerprint));
    }

    #[test]
    fn foreign_records_are_lenient() {
        // e.g. a macOS Screen Sharing or NAS record with no Farsight keys.
        let info = parse_txt(pairs(&[("txtvers", Some("1")), ("model", Some("NAS"))]));
        assert_eq!(info, TxtInfo::default());
    }

    #[test]
    fn malformed_values_are_dropped() {
        let info = parse_txt(pairs(&[
            ("farsight", Some("yes")),
            ("v", Some("3.8; rm -rf")),
            ("fp", Some("deadbeef")),
        ]));
        assert_eq!(info, TxtInfo::default());
        let info = parse_txt(pairs(&[("farsight", None), ("v", None), ("fp", None)]));
        assert_eq!(info, TxtInfo::default());
    }

    #[test]
    fn keys_are_case_insensitive_and_first_wins() {
        let info = parse_txt(pairs(&[
            ("FARSIGHT", Some("1")),
            ("V", Some("3.7")),
            ("v", Some("3.8")),
            ("Fp", Some(FP)),
        ]));
        assert!(info.farsight);
        assert_eq!(info.version.as_deref(), Some("3.7"));
        assert_eq!(info.fingerprint.as_deref(), Some(FP));
    }

    #[test]
    fn invalid_utf8_is_ignored() {
        let bad: &[u8] = &[0xff, 0xfe];
        let info = parse_txt([("v", Some(bad))]);
        assert_eq!(info.version, None);
    }

    proptest! {
        #[test]
        fn parse_never_panics(entries in prop::collection::vec(
            ("\\PC{0,8}", prop::option::of(prop::collection::vec(any::<u8>(), 0..80))), 0..10)) {
            let info = parse_txt(entries.iter().map(|(k, v)| (k.as_str(), v.as_deref())));
            if let Some(fp) = info.fingerprint {
                prop_assert_eq!(fp.len(), FINGERPRINT_HEX_LEN);
            }
            if let Some(v) = info.version {
                prop_assert!(v.len() <= MAX_VERSION_LEN);
            }
        }
    }
}
