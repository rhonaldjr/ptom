//! The agent's long-term identity: a self-signed TLS certificate (VeNCrypt
//! X509*) and an RSA key (RA2). Viewers pin their fingerprints (ADR-0017).

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use farsight_security::ra2::Ra2ServerKey;
use sha2::{Digest, Sha256};

/// Files under `$XDG_CONFIG_HOME/farsight/agent/`.
const CERT_FILE: &str = "tls-cert.pem";
const KEY_FILE: &str = "tls-key.pem";
const RSA_FILE: &str = "ra2-key.pk8";
/// The Wayland portal's remembered approval (U-3).
pub const PORTAL_TOKEN_FILE: &str = "portal-token";

/// The agent's certificate chain and keys.
pub struct Identity {
    /// PEM certificate.
    pub cert_pem: String,
    /// PEM private key for the certificate.
    pub key_pem: String,
    /// The RA2 key.
    pub ra2: Arc<Ra2ServerKey>,
}

impl Identity {
    /// Loads the identity from `dir`, creating it on first run.
    pub fn load_or_create(dir: &Path, host_name: &str) -> Result<Self> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        // DirBuilder's mode only applies when it creates the directory.
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("restricting {}", dir.display()))?;
        }
        let (cert_path, key_path) = (dir.join(CERT_FILE), dir.join(KEY_FILE));
        // Never replace an identity silently: every viewer would then see
        // "identity changed" and learn to click through it. Generate only on
        // first run (both files absent); anything else is an error to fix.
        let (cert_pem, key_pem) = match (cert_path.exists(), key_path.exists()) {
            (false, false) => {
                let names = vec![host_name.to_owned(), "localhost".to_owned()];
                let generated = rcgen::generate_simple_self_signed(names)
                    .context("generating the TLS certificate")?;
                let cert = generated.cert.pem();
                let key = generated.signing_key.serialize_pem();
                write_private(&key_path, key.as_bytes())?;
                write_private(&cert_path, cert.as_bytes())?;
                (cert, key)
            }
            _ => (
                fs::read_to_string(&cert_path)
                    .with_context(|| format!("reading {}", cert_path.display()))?,
                fs::read_to_string(&key_path)
                    .with_context(|| format!("reading {}", key_path.display()))?,
            ),
        };
        let rsa_path = dir.join(RSA_FILE);
        let ra2 = if rsa_path.exists() {
            let der =
                fs::read(&rsa_path).with_context(|| format!("reading {}", rsa_path.display()))?;
            Ra2ServerKey::from_pkcs8(&der).map_err(|e| anyhow::anyhow!(e.reason))?
        } else {
            let key = Ra2ServerKey::generate().map_err(|e| anyhow::anyhow!(e.reason))?;
            let der = key.to_pkcs8().map_err(|e| anyhow::anyhow!(e.reason))?;
            write_private(&rsa_path, &der)?;
            key
        };
        Ok(Self {
            cert_pem,
            key_pem,
            ra2: Arc::new(ra2),
        })
    }

    /// The default directory.
    pub fn default_dir() -> Result<PathBuf> {
        Ok(farsight_vault::paths::config_home()
            .context("no config directory")?
            .join("farsight")
            .join("agent"))
    }

    /// SHA-256 of the DER certificate, as viewers display it.
    pub fn tls_fingerprint(&self) -> Result<[u8; 32]> {
        let der = pem_body(&self.cert_pem).context("certificate is not PEM")?;
        Ok(Sha256::digest(der).into())
    }
}

/// Formats a fingerprint as the viewer shows it (two lines of 16 bytes).
pub fn format_fingerprint(fingerprint: &[u8; 32]) -> String {
    fingerprint
        .chunks(16)
        .map(|half| {
            half.iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Writes a file only the owner can read (atomic replace).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)
        .with_context(|| format!("writing {}", temp.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    Ok(())
}

/// Decodes the base64 body of the first PEM block.
fn pem_body(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    base64_decode(&body)
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
    {
        let value = u32::try_from(ALPHABET.iter().position(|&a| a == c)?).ok()?;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_created_once_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path(), "test-host").unwrap();
        let second = Identity::load_or_create(dir.path(), "test-host").unwrap();
        assert_eq!(
            first.tls_fingerprint().unwrap(),
            second.tls_fingerprint().unwrap()
        );
        assert_eq!(first.ra2.fingerprint(), second.ra2.fingerprint());
        for file in [KEY_FILE, RSA_FILE] {
            let mode = fs::metadata(dir.path().join(file))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }

    #[test]
    fn a_half_missing_identity_is_an_error_not_a_silent_replacement() {
        let dir = tempfile::tempdir().unwrap();
        Identity::load_or_create(dir.path(), "test-host").unwrap();
        fs::remove_file(dir.path().join(KEY_FILE)).unwrap();
        assert!(Identity::load_or_create(dir.path(), "test-host").is_err());
    }

    #[test]
    fn base64_decodes_standard_text() {
        assert_eq!(base64_decode("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(base64_decode("not*base64"), None);
    }
}
