//! Open H.264 for the agent (M10), used only with `--h264`: the GPU's
//! encoder when there is one (`vaapi`/`nvenc` features, U-15), otherwise Cisco's
//! OpenH264, downloaded on request (`h264` feature, U-12). Both are off in
//! release builds until tested by hand (U-14).

use std::sync::Arc;

use anyhow::Result;
use farsight_codec::video::{LimitedCodecs, VideoCodecs};

/// Where Farsight keeps downloaded libraries.
#[cfg(feature = "h264")]
fn data_dir() -> Result<std::path::PathBuf> {
    use anyhow::Context;
    Ok(farsight_vault::paths::data_home()
        .context("no data directory")?
        .join("farsight"))
}

/// `--install-openh264`.
#[cfg(feature = "h264")]
pub fn install() -> Result<()> {
    let path = farsight_h264::install(&data_dir()?)?;
    println!("Installed {}.\n{}", path.display(), farsight_h264::NOTICE);
    Ok(())
}

#[cfg(not(feature = "h264"))]
pub fn install() -> Result<()> {
    anyhow::bail!("this build has no H.264 support (it needs the `h264` feature)")
}

/// The codecs for `--h264`, or `None` without it.
///
/// # Errors
/// When H.264 was asked for but neither a GPU encoder nor OpenH264 is here.
pub fn codecs(wanted: bool) -> Result<Option<Arc<dyn VideoCodecs>>> {
    if !wanted {
        return Ok(None);
    }
    let software = software()?;
    let codecs = gpu(software.clone()).or(software).ok_or_else(|| {
        anyhow::anyhow!(
            "H.264 isn't available: no GPU encoder here, and OpenH264 isn't installed \
             (run with --install-openh264, in a build with the h264 feature)"
        )
    })?;
    // Each stream is a GPU session or a busy CPU core: at most a few at once.
    Ok(Some(Arc::new(LimitedCodecs::new(codecs, MAX_STREAMS))))
}

/// H.264 streams at once (viewers beyond it get lossless updates).
const MAX_STREAMS: usize = 4;

/// Cisco's OpenH264, if this build has it and it's installed.
#[cfg(feature = "h264")]
fn software() -> Result<Option<Arc<dyn VideoCodecs>>> {
    match farsight_h264::OpenH264::load(&data_dir()?) {
        Ok(codecs) => {
            tracing::info!("{}", farsight_h264::NOTICE);
            Ok(Some(Arc::new(codecs)))
        }
        Err(farsight_h264::H264Error::NotInstalled) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(feature = "h264"))]
#[allow(clippy::unnecessary_wraps)] // Same shape as the h264 build.
fn software() -> Result<Option<Arc<dyn VideoCodecs>>> {
    Ok(None)
}

/// The GPU's encoder (falling back to `software` for anything it can't do).
#[cfg(any(feature = "vaapi", feature = "nvenc"))]
fn gpu(software: Option<Arc<dyn VideoCodecs>>) -> Option<Arc<dyn VideoCodecs>> {
    let gpu = farsight_hwenc::detect()?;
    tracing::info!("H.264 on the GPU: {gpu}");
    Some(Arc::new(farsight_hwenc::HardwareCodecs::new(gpu, software)))
}

#[cfg(not(any(feature = "vaapi", feature = "nvenc")))]
fn gpu(_software: Option<Arc<dyn VideoCodecs>>) -> Option<Arc<dyn VideoCodecs>> {
    None
}

/// `--check-h264`: which encoder `--h264` would use, how fast it encodes
/// 1080p, and (with OpenH264 installed) whether a frame decodes correctly.
/// For checking a new computer or GPU by hand (U-14).
pub fn check() -> Result<()> {
    use anyhow::Context;
    let codecs = codecs(true)?.context("no H.264 encoder")?;
    let (width, height) = (1920u16, 1080u16);
    let mut encoder = codecs
        .encoder(width, height)
        .context("starting the encoder")?;
    let still = test_picture(width, height, 0);
    let moving: Vec<Vec<u32>> = (1..4).map(|n| test_picture(width, height, n * 8)).collect();
    let first = encoder.encode(&still).context("encoding")?;
    match software()?.map(|s| s.decoder()).transpose()? {
        Some(mut decoder) => {
            let picture = decoder
                .decode(&first, (width, height))?
                .context("the first frame didn't decode")?;
            let error = average_error(&still, &picture.pixels);
            println!(
                "Picture check: {} (average colour error {error})",
                if error < 6 { "OK" } else { "POOR" }
            );
        }
        None => println!("Picture check: skipped (OpenH264 isn't installed to decode with)"),
    }
    let frames = 120;
    let start = std::time::Instant::now();
    let mut bytes = 0;
    for picture in moving.iter().cycle().take(frames) {
        bytes += encoder.encode(picture).context("encoding")?.len();
    }
    let seconds = start.elapsed().as_secs_f64();
    #[allow(clippy::cast_precision_loss)]
    let (rate, megabits) = (frames as f64 / seconds, bytes as f64 * 8.0 / seconds / 1e6);
    println!("Speed: {rate:.0} frames per second at {width}x{height} ({megabits:.1} Mbit/s)");
    Ok(())
}

/// A checkerboard of 32-pixel squares moved `shift` (even) pixels: edges
/// stay on 2×2 boundaries, so 4:2:0 colour doesn't blur them and the check
/// measures the encoder.
fn test_picture(width: u16, height: u16, shift: usize) -> Vec<u32> {
    let w = usize::from(width);
    (0..w * usize::from(height))
        .map(|i| {
            if ((i % w + shift) / 32 + i / w / 32).is_multiple_of(2) {
                0x00d0_3030
            } else {
                0x0030_60d0
            }
        })
        .collect()
}

fn average_error(a: &[u32], b: &[u32]) -> u64 {
    let total: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| {
            let (x, y) = (x.to_be_bytes(), y.to_be_bytes());
            x.iter()
                .zip(y)
                .map(|(&p, q)| u64::from(p.abs_diff(q)))
                .sum::<u64>()
        })
        .sum();
    total / a.len().max(1) as u64
}
