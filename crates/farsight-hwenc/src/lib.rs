//! Hardware H.264 encoding for the agent (roadmap M10, user decision U-15):
//! VA-API for AMD and Intel GPUs (`vaapi` feature) and NVENC for NVIDIA
//! (`nvenc`); both load their libraries at run time. [`HardwareCodecs`]
//! tries the GPU first and falls back to software (Cisco's OpenH264) for
//! anything the GPU can't do; decoding is always software.
//!
//! Layer: **Adapters** (ADR-0002). Off in release builds until hand-tested
//! (U-14).

pub mod nv12;
#[cfg(feature = "nvenc")]
mod nvenc;
pub mod syntax;
#[cfg(feature = "vaapi")]
mod vaapi;
#[cfg(any(feature = "vaapi", feature = "nvenc"))]
mod worker;

use std::sync::Arc;

use farsight_codec::video::{VideoCodecs, VideoDecoder, VideoEncoder, VideoError};

/// The bit rate hardware encoders aim for (enough for crisp 1080p text).
pub const BITRATE_BPS: u32 = 8_000_000;
/// Frames per second the rate control plans for.
pub const FRAME_RATE: u32 = 30;
/// A keyframe at least this often, so a lost frame heals quickly.
pub const KEYFRAME_INTERVAL: u16 = 300;

/// A GPU video encoder this computer has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gpu {
    /// AMD or Intel, through VA-API.
    VaApi,
    /// NVIDIA, through NVENC.
    Nvenc,
}

impl std::fmt::Display for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::VaApi => "VA-API (AMD/Intel)",
            Self::Nvenc => "NVENC (NVIDIA)",
        })
    }
}

/// Finds a GPU that can encode H.264, if this build supports one.
#[must_use]
pub fn detect() -> Option<Gpu> {
    // NVIDIA first: its VA-API driver (if any) only decodes.
    #[cfg(feature = "nvenc")]
    if nvenc::available() {
        return Some(Gpu::Nvenc);
    }
    #[cfg(feature = "vaapi")]
    if vaapi::available() {
        return Some(Gpu::VaApi);
    }
    None
}

/// Encoders on `gpu`, falling back to `software` (also used for decoding).
pub struct HardwareCodecs {
    gpu: Gpu,
    software: Option<Arc<dyn VideoCodecs>>,
}

impl HardwareCodecs {
    /// Codecs that encode on `gpu`.
    #[must_use]
    pub fn new(gpu: Gpu, software: Option<Arc<dyn VideoCodecs>>) -> Self {
        Self { gpu, software }
    }

    fn gpu_encoder(&self, width: u16, height: u16) -> Result<Box<dyn VideoEncoder>, VideoError> {
        match self.gpu {
            #[cfg(feature = "vaapi")]
            Gpu::VaApi => vaapi::encoder(width, height),
            #[cfg(feature = "nvenc")]
            Gpu::Nvenc => nvenc::encoder(width, height),
            #[allow(unreachable_patterns)]
            gpu => {
                let _ = (width, height);
                Err(VideoError(format!("this build can't use {gpu}")))
            }
        }
    }
}

impl VideoCodecs for HardwareCodecs {
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, VideoError> {
        self.software
            .as_ref()
            .ok_or_else(|| VideoError("no H.264 decoder installed".into()))?
            .decoder()
    }

    fn encoder(&self, width: u16, height: u16) -> Result<Box<dyn VideoEncoder>, VideoError> {
        match self.gpu_encoder(width, height) {
            Ok(encoder) => Ok(encoder),
            Err(problem) => {
                tracing::warn!(
                    "{} can't encode {width}x{height} ({problem}); using software",
                    self.gpu
                );
                self.software
                    .as_ref()
                    .ok_or(problem)?
                    .encoder(width, height)
            }
        }
    }
}
