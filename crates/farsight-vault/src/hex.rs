//! Lower-case hex for identifiers, fingerprints and sealed blobs.

use std::fmt::Write as _;

pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Writing to a String cannot fail.
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|&[hi, lo]| Some((nibble(hi)? << 4) | nibble(lo)?))
        .collect()
}

pub(crate) fn decode_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    decode(text)?.try_into().ok()
}

/// Serde adapter for fixed-size byte arrays as hex strings.
pub(crate) mod array {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub(crate) fn serialize<S: Serializer, const N: usize>(
        bytes: &[u8; N],
        ser: S,
    ) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&super::encode(bytes))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        de: D,
    ) -> Result<[u8; N], D::Error> {
        let text = String::deserialize(de)?;
        super::decode_array(&text).ok_or_else(|| D::Error::custom("invalid hex value"))
    }
}

/// Serde adapter for byte vectors as hex strings.
pub(crate) mod vec {
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    // Serde's `serialize_with` passes `&Vec<u8>`.
    #[allow(clippy::ptr_arg)]
    pub(crate) fn serialize<S: Serializer>(bytes: &Vec<u8>, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&super::encode(bytes))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(de)?;
        super::decode(&text).ok_or_else(|| D::Error::custom("invalid hex value"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(encode(&[0x00, 0xab, 0xff]), "00abff");
        assert_eq!(decode("00abFF"), Some(vec![0x00, 0xab, 0xff]));
        assert_eq!(decode("abc"), None);
        assert_eq!(decode("zz"), None);
        assert_eq!(decode_array::<2>("0102"), Some([1, 2]));
        assert_eq!(decode_array::<2>("010203"), None);
    }
}
