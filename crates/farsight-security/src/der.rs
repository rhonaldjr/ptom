//! Minimal DER for RSA public keys: converting between RA2's raw
//! (modulus, exponent) form and X.509 `SubjectPublicKeyInfo`, which is what
//! aws-lc-rs accepts and produces.

/// DER encoding of the `rsaEncryption` algorithm identifier
/// (`SEQUENCE { OID 1.2.840.113549.1.1.1, NULL }`).
const RSA_ALGORITHM: [u8; 15] = [
    0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00,
];

const TAG_INTEGER: u8 = 0x02;
const TAG_BIT_STRING: u8 = 0x03;
const TAG_SEQUENCE: u8 = 0x30;

/// Builds a `SubjectPublicKeyInfo` for the big-endian `modulus` and `exponent`.
#[must_use]
pub fn rsa_spki(modulus: &[u8], exponent: &[u8]) -> Vec<u8> {
    let mut key = Vec::new();
    put_integer(&mut key, modulus);
    put_integer(&mut key, exponent);
    let rsa_public_key = tlv(TAG_SEQUENCE, &key);
    let mut bits = vec![0u8]; // no unused bits
    bits.extend_from_slice(&rsa_public_key);
    let mut body = RSA_ALGORITHM.to_vec();
    body.extend_from_slice(&tlv(TAG_BIT_STRING, &bits));
    tlv(TAG_SEQUENCE, &body)
}

/// Extracts (modulus, exponent) from an RSA `SubjectPublicKeyInfo`, without
/// leading zero bytes. Returns `None` if the input is not one.
#[must_use]
pub fn rsa_components(spki: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (body, rest) = read_tlv(spki, TAG_SEQUENCE)?;
    if !rest.is_empty() {
        return None;
    }
    let algorithm = body.get(..RSA_ALGORITHM.len())?;
    if algorithm != RSA_ALGORITHM {
        return None;
    }
    let (bits, rest) = read_tlv(body.get(RSA_ALGORITHM.len()..)?, TAG_BIT_STRING)?;
    let (&unused, key) = bits.split_first()?;
    if unused != 0 || !rest.is_empty() {
        return None;
    }
    let (key, rest) = read_tlv(key, TAG_SEQUENCE)?;
    if !rest.is_empty() {
        return None;
    }
    let (modulus, key) = read_tlv(key, TAG_INTEGER)?;
    let (exponent, rest) = read_tlv(key, TAG_INTEGER)?;
    rest.is_empty().then(|| {
        (
            strip_zeros(modulus).to_vec(),
            strip_zeros(exponent).to_vec(),
        )
    })
}

/// Left-pads `value` with zeros to `len` bytes (RA2 sends both key parts at
/// the modulus length). Returns `None` if it doesn't fit.
#[must_use]
pub fn pad_to(value: &[u8], len: usize) -> Option<Vec<u8>> {
    let value = strip_zeros(value);
    let padding = len.checked_sub(value.len())?;
    let mut out = vec![0u8; padding];
    out.extend_from_slice(value);
    Some(out)
}

fn strip_zeros(value: &[u8]) -> &[u8] {
    let first = value.iter().position(|&b| b != 0).unwrap_or(value.len());
    value.get(first..).unwrap_or_default()
}

fn put_integer(out: &mut Vec<u8>, value: &[u8]) {
    let value = strip_zeros(value);
    let mut content = Vec::with_capacity(value.len() + 1);
    if value.first().is_none_or(|&b| b & 0x80 != 0) {
        content.push(0);
    }
    content.extend_from_slice(value);
    out.extend_from_slice(&tlv(TAG_INTEGER, &content));
}

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(u8::try_from(len).unwrap_or(0));
    } else {
        let bytes = len.to_be_bytes();
        let significant = strip_zeros(&bytes);
        out.push(0x80 | u8::try_from(significant.len()).unwrap_or(0));
        out.extend_from_slice(significant);
    }
    out.extend_from_slice(content);
    out
}

/// Reads one TLV with the expected tag; returns (content, remainder).
fn read_tlv(input: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    let (&actual, rest) = input.split_first()?;
    if actual != tag {
        return None;
    }
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 {
            return None;
        }
        let len_bytes = rest.get(..count)?;
        let len = len_bytes
            .iter()
            .fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
        (len, rest.get(count..)?)
    };
    Some((rest.get(..len)?, rest.get(len..)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn round_trips_a_key_with_high_bit_modulus() {
        let modulus = vec![0xc1; 256];
        let exponent = [0x01, 0x00, 0x01];
        let spki = rsa_spki(&modulus, &exponent);
        assert_eq!(rsa_components(&spki), Some((modulus, exponent.to_vec())));
    }

    #[test]
    fn padding_is_stripped_and_restored() {
        assert_eq!(pad_to(&[0, 0, 1, 0, 1], 4), Some(vec![0, 1, 0, 1]));
        assert_eq!(pad_to(&[1, 2, 3], 2), None);
    }

    #[test]
    fn rejects_non_rsa_input() {
        assert_eq!(rsa_components(&[0x30, 0x03, 0x02, 0x01, 0x01]), None);
        assert_eq!(rsa_components(&[]), None);
    }

    proptest! {
        #[test]
        fn parsing_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
            let _ = rsa_components(&bytes);
        }

        #[test]
        fn arbitrary_components_round_trip(
            modulus in proptest::collection::vec(1u8.., 1..300),
            exponent in proptest::collection::vec(1u8.., 1..8),
        ) {
            let spki = rsa_spki(&modulus, &exponent);
            prop_assert_eq!(rsa_components(&spki), Some((modulus, exponent)));
        }
    }
}
