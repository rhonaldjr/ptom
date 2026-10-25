//! H.264 on AMD and Intel GPUs through VA-API. The GPU work happens in
//! `farsight-gpu-sys` (libva, loaded at run time); this side picks the
//! device, plans keyframes and writes the stream's headers.

use std::path::PathBuf;

use farsight_codec::video::{VideoEncoder, VideoError};
use farsight_gpu_sys::va::{Entrypoint, Frame, H264Encoder, Packed, Sequence, Va};

use crate::syntax::{SliceFrame, Stream};
use crate::worker;
use crate::{BITRATE_BPS, FRAME_RATE, KEYFRAME_INTERVAL, nv12};

/// Whether a GPU here can encode H.264 through VA-API.
pub(crate) fn available() -> bool {
    encoding_device().is_some()
}

/// The first GPU (render node) whose VA-API driver encodes H.264.
fn encoding_device() -> Option<(PathBuf, Va, Entrypoint)> {
    let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("renderD"))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().find_map(|node| {
        let va = Va::open(&node).ok()?;
        let entrypoint = va.h264_encoder()?;
        Some((node, va, entrypoint))
    })
}

fn failed(error: impl std::fmt::Display) -> VideoError {
    VideoError(error.to_string())
}

/// A new encoder for `width × height` frames, running on its own thread.
pub(crate) fn encoder(width: u16, height: u16) -> Result<Box<dyn VideoEncoder>, VideoError> {
    worker::spawn("farsight-vaapi", move || Session::new(width, height))
}

/// One encoding session, owned by its thread.
struct Session {
    encoder: H264Encoder,
    stream: Stream,
    /// Frames since the last keyframe.
    since_keyframe: u32,
    idr_pic_id: u16,
}

impl worker::Session for Session {
    fn encode(&mut self, pixels: &[u32]) -> Result<Vec<u8>, VideoError> {
        self.encode_frame(pixels)
    }
}

impl Session {
    fn new(width: u16, height: u16) -> Result<Self, VideoError> {
        let stream = Stream::new(width, height)
            .ok_or_else(|| VideoError(format!("{width}x{height} is too large for H.264")))?;
        let (node, va, entrypoint) =
            encoding_device().ok_or_else(|| VideoError("no GPU here encodes H.264".into()))?;
        let sequence = Sequence {
            width_mbs: stream.width_mbs(),
            height_mbs: stream.height_mbs(),
            crop_right: stream.crop_right(),
            crop_bottom: stream.crop_bottom(),
            level_idc: stream.level_idc,
            keyframe_interval: u32::from(KEYFRAME_INTERVAL),
            log2_max_frame_num_minus4: stream.log2_max_frame_num_minus4,
            bits_per_second: BITRATE_BPS,
            frame_rate: FRAME_RATE,
            initial_qp: stream.initial_qp,
        };
        let encoder = H264Encoder::new(va, entrypoint, sequence).map_err(failed)?;
        let mode = if encoder.constant_bitrate() {
            "constant bit rate"
        } else {
            "constant quality"
        };
        tracing::info!(
            "hardware H.264 encoding on {} ({entrypoint:?}, {mode})",
            node.display()
        );
        Ok(Self {
            encoder,
            stream,
            since_keyframe: 0,
            idr_pic_id: 0,
        })
    }

    fn encode_frame(&mut self, pixels: &[u32]) -> Result<Vec<u8>, VideoError> {
        let s = self.stream;
        let picture = nv12::from_xrgb(pixels, usize::from(s.width), usize::from(s.height))
            .and_then(|p| {
                p.padded(
                    usize::from(s.width_mbs()) * 16,
                    usize::from(s.height_mbs()) * 16,
                )
            })
            .ok_or_else(|| VideoError("frame size changed".into()))?;
        let idr = self.since_keyframe == 0;
        let wrap = 1u32 << (s.log2_max_frame_num_minus4 + 4);
        let frame_num = u16::try_from(self.since_keyframe % wrap).unwrap_or(0);
        let (slice_header, slice_bits) = s.slice_header(SliceFrame {
            idr,
            frame_num,
            idr_pic_id: self.idr_pic_id,
        });
        let mut headers = Vec::with_capacity(2);
        if idr {
            headers.push(Packed::Sequence(s.sequence_parameter_set()));
            headers.push(Packed::Picture(s.picture_parameter_set()));
        }
        headers.push(Packed::Slice(slice_header, slice_bits));
        let frame = Frame {
            idr,
            frame_num,
            idr_pic_id: self.idr_pic_id,
        };
        let coded = self
            .encoder
            .encode(&picture.data, frame, &headers)
            .map_err(failed)?;
        self.since_keyframe = (self.since_keyframe + 1) % u32::from(KEYFRAME_INTERVAL);
        if idr {
            self.idr_pic_id = self.idr_pic_id.wrapping_add(1);
        }
        Ok(coded)
    }
}
