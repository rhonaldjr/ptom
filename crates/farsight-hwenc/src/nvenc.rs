//! H.264 on NVIDIA GPUs through NVENC. The GPU work happens in
//! `farsight-gpu-sys` (the NVIDIA driver's libraries, loaded at run time);
//! NVENC writes the whole stream, so this side only converts colours.

use farsight_codec::video::{VideoEncoder, VideoError};
use farsight_gpu_sys::nvenc::{NvencEncoder, Settings};

use crate::worker;
use crate::{BITRATE_BPS, FRAME_RATE, KEYFRAME_INTERVAL, nv12};

/// Whether an NVIDIA GPU here can encode H.264.
pub(crate) fn available() -> bool {
    farsight_gpu_sys::nvenc::available()
}

/// A new encoder for `width × height` frames, running on its own thread.
pub(crate) fn encoder(width: u16, height: u16) -> Result<Box<dyn VideoEncoder>, VideoError> {
    worker::spawn("farsight-nvenc", move || {
        let encoder = NvencEncoder::new(Settings {
            width: u32::from(width),
            height: u32::from(height),
            bits_per_second: BITRATE_BPS,
            frame_rate: FRAME_RATE,
            keyframe_interval: u32::from(KEYFRAME_INTERVAL),
        })
        .map_err(|e| VideoError(e.to_string()))?;
        tracing::info!("hardware H.264 encoding on the NVIDIA GPU (NVENC)");
        Ok(Session {
            encoder,
            width: usize::from(width),
            height: usize::from(height),
        })
    })
}

struct Session {
    encoder: NvencEncoder,
    width: usize,
    height: usize,
}

impl worker::Session for Session {
    fn encode(&mut self, pixels: &[u32]) -> Result<Vec<u8>, VideoError> {
        let picture = nv12::from_xrgb(pixels, self.width, self.height)
            .ok_or_else(|| VideoError("frame size changed".into()))?;
        self.encoder
            .encode(&picture.data)
            .map_err(|e| VideoError(e.to_string()))
    }
}
