//! Open H.264 for Farsight (roadmap M10, user decision U-12) using **Cisco's
//! prebuilt OpenH264 library**, which Cisco licenses (and pays the H.264
//! patent royalties for) when users download it from Cisco. Farsight never
//! builds or ships the codec: [`install`] fetches Cisco's binary on the
//! user's request, and [`OpenH264::load`] checks its SHA-256 against the
//! builds this version knows before loading it.
//!
//! Notice (required by Cisco): "OpenH264 Video Codec provided by Cisco
//! Systems, Inc." — see [`NOTICE`].

mod codec;
mod download;

use std::path::{Path, PathBuf};

pub use codec::OpenH264;

/// The attribution Cisco's licence asks for, shown wherever H.264 is offered.
pub const NOTICE: &str = "OpenH264 Video Codec provided by Cisco Systems, Inc.";

/// Cisco's library for this computer: file name and its SHA-256.
#[derive(Debug, Clone, Copy)]
struct Build {
    file: &'static str,
    sha256: &'static str,
}

/// OpenH264 2.6.0 builds (hashes checked against Cisco's downloads, and
/// also listed by the `openh264` crate, which re-checks before loading).
const BUILDS: &[(&str, Build)] = &[
    (
        "x86_64",
        Build {
            file: "libopenh264-2.6.0-linux64.8.so",
            sha256: "2f0cde7c6a6abcf5cae76942894ea42897fa677bce4ed6c91a24dd1b041d5f04",
        },
    ),
    (
        "aarch64",
        Build {
            file: "libopenh264-2.6.0-linux-arm64.8.so",
            sha256: "12e7b33623667cdab0e575170c147b1b36eadb77d0d2aa7ceb5afd3e58902140",
        },
    ),
];

/// Cisco's download server.
const CISCO_HOST: &str = "ciscobinary.openh264.org";

/// Why H.264 isn't available.
#[derive(Debug, thiserror::Error)]
pub enum H264Error {
    /// Cisco has no build for this kind of computer.
    #[error("Cisco's OpenH264 isn't available for this kind of computer")]
    UnsupportedPlatform,
    /// The library hasn't been installed yet.
    #[error("OpenH264 isn't installed; run with --install-openh264 first")]
    NotInstalled,
    /// Downloading failed.
    #[error("downloading OpenH264 failed: {0}")]
    Download(String),
    /// The download (or the installed file) isn't the build we expect.
    #[error("the OpenH264 library doesn't match Cisco's published build")]
    Mismatch,
    /// File system trouble.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The library couldn't be loaded.
    #[error("could not load OpenH264: {0}")]
    Load(String),
}

fn build() -> Result<Build, H264Error> {
    BUILDS
        .iter()
        .find(|(arch, _)| *arch == std::env::consts::ARCH)
        .map(|(_, build)| *build)
        .filter(|_| cfg!(target_os = "linux"))
        .ok_or(H264Error::UnsupportedPlatform)
}

/// Where the library lives once installed (`<data_dir>/openh264/<file>`).
///
/// # Errors
/// [`H264Error::UnsupportedPlatform`] when Cisco has no build for here.
pub fn library_path(data_dir: &Path) -> Result<PathBuf, H264Error> {
    Ok(data_dir.join("openh264").join(build()?.file))
}

/// Downloads Cisco's library into `data_dir` (over HTTPS, then checks its
/// SHA-256). Does nothing if the right file is already there.
///
/// # Errors
/// [`H264Error`] on network, checksum or file system problems.
pub fn install(data_dir: &Path) -> Result<PathBuf, H264Error> {
    let build = build()?;
    let path = library_path(data_dir)?;
    if std::fs::read(&path).is_ok_and(|bytes| sha256_hex(&bytes) == build.sha256) {
        return Ok(path);
    }
    let compressed = download::https_get(CISCO_HOST, &format!("/{}.bz2", build.file))?;
    let mut library = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(bzip2::read::BzDecoder::new(&compressed[..]), 64 << 20),
        &mut library,
    )?;
    if sha256_hex(&library) != build.sha256 {
        return Err(H264Error::Mismatch);
    }
    let dir = path.parent().ok_or(H264Error::NotInstalled)?;
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".{}.part", build.file));
    std::fs::write(&temporary, &library)?;
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}

/// Checks that `path` is the pinned build for this computer.
pub(crate) fn verify(path: &Path) -> Result<(), H264Error> {
    let build = build()?;
    if sha256_hex(&std::fs::read(path)?) == build.sha256 {
        Ok(())
    } else {
        Err(H264Error::Mismatch)
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_pinned_build_passes_verification() {
        let dir = std::env::temp_dir().join(format!("farsight-h264-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.so");
        std::fs::write(&path, b"not cisco's library").unwrap();
        if build().is_ok() {
            assert!(matches!(verify(&path), Err(H264Error::Mismatch)));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_known_build_has_a_full_hash() {
        for (_, build) in BUILDS {
            assert_eq!(build.sha256.len(), 64);
            assert!(build.file.starts_with("libopenh264-"));
        }
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
