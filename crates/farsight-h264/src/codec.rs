//! [`farsight_codec::video`] ports implemented with Cisco's OpenH264.

use std::path::PathBuf;

use farsight_codec::video::{Picture, VideoCodecs, VideoDecoder, VideoEncoder, VideoError};
use openh264::OpenH264API;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, UsageType};
use openh264::formats::{ArgbSliceU32, YUVBuffer, YUVSource};

use crate::H264Error;

/// Bit rate for screen content (enough for crisp 1080p text).
const BITRATE_BPS: u32 = 8_000_000;
const MAX_FRAME_RATE: f32 = 30.0;

/// Codecs backed by the installed Cisco library.
#[derive(Debug, Clone)]
pub struct OpenH264 {
    library: PathBuf,
}

impl OpenH264 {
    /// Uses the library installed in `data_dir`, checking it first.
    ///
    /// # Errors
    /// [`H264Error::NotInstalled`], or a load/checksum failure.
    pub fn load(data_dir: &std::path::Path) -> Result<Self, H264Error> {
        let library = crate::library_path(data_dir)?;
        if !library.exists() {
            return Err(H264Error::NotInstalled);
        }
        let codecs = Self { library };
        // Fail now, not on the first frame.
        codecs.api().map_err(|e| H264Error::Load(e.0))?;
        Ok(codecs)
    }

    /// Loads the library after checking it is exactly the build Farsight
    /// pins for this computer (the `openh264` crate would also accept older
    /// Cisco builds, some with known decoder flaws). The file lives in the
    /// user's own data folder, so the gap between this check and the load
    /// needs the user's own write access (accepted, threat model).
    fn api(&self) -> Result<OpenH264API, VideoError> {
        crate::verify(&self.library).map_err(|e| VideoError(e.to_string()))?;
        OpenH264API::from_blob_path(&self.library).map_err(|e| VideoError(e.to_string()))
    }
}

impl VideoCodecs for OpenH264 {
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, VideoError> {
        let decoder = Decoder::with_api_config(self.api()?, DecoderConfig::new())
            .map_err(|e| VideoError(e.to_string()))?;
        Ok(Box::new(H264Decoder(decoder)))
    }

    fn encoder(&self, width: u16, height: u16) -> Result<Box<dyn VideoEncoder>, VideoError> {
        if !width.is_multiple_of(2) || !height.is_multiple_of(2) || width == 0 || height == 0 {
            return Err(VideoError("H.264 frames need even, non-zero sizes".into()));
        }
        let config = EncoderConfig::new()
            .usage_type(UsageType::ScreenContentRealTime)
            .bitrate(BitRate::from_bps(BITRATE_BPS))
            .max_frame_rate(FrameRate::from_hz(MAX_FRAME_RATE));
        let encoder =
            Encoder::with_api_config(self.api()?, config).map_err(|e| VideoError(e.to_string()))?;
        Ok(Box::new(H264Encoder {
            encoder,
            width: usize::from(width),
            height: usize::from(height),
        }))
    }
}

struct H264Encoder {
    encoder: Encoder,
    width: usize,
    height: usize,
}

impl VideoEncoder for H264Encoder {
    fn encode(&mut self, pixels: &[u32]) -> Result<Vec<u8>, VideoError> {
        if pixels.len() != self.width * self.height {
            return Err(VideoError("frame size changed".into()));
        }
        // 0x00RRGGBB is the crate's "ARGB" layout (alpha ignored).
        let yuv = YUVBuffer::from_rgb_source(ArgbSliceU32::new(pixels, (self.width, self.height)));
        let stream = self
            .encoder
            .encode(&yuv)
            .map_err(|e| VideoError(e.to_string()))?;
        Ok(stream.to_vec())
    }
}

/// One rectangle's decoder.
struct H264Decoder(Decoder);

impl VideoDecoder for H264Decoder {
    fn decode(&mut self, data: &[u8], size: (u16, u16)) -> Result<Option<Picture>, VideoError> {
        let expected = farsight_codec::video::coded_size(size);
        let mut last = None;
        let mut pictures = 0;
        for packet in openh264::nal_units(data) {
            let Some(yuv) = self
                .0
                .decode(packet)
                .map_err(|e| VideoError(e.to_string()))?
            else {
                continue;
            };
            pictures += 1;
            if pictures > farsight_codec::video::MAX_PICTURES_PER_RECT {
                return Err(VideoError("too many pictures in one rectangle".into()));
            }
            // Checked before converting: no allocation for a size we didn't expect.
            let (width, height) = yuv.dimensions();
            if (width, height) != expected {
                return Err(VideoError(format!(
                    "picture is {width}x{height}, expected {}x{}",
                    expected.0, expected.1
                )));
            }
            let mut rgb = vec![0u8; width * height * 3];
            yuv.write_rgb8(&mut rgb);
            let pixels = rgb
                .as_chunks::<3>()
                .0
                .iter()
                .map(|[r, g, b]| u32::from_be_bytes([0, *r, *g, *b]))
                .collect();
            last = Some(Picture {
                width: u16::try_from(width).map_err(|_| VideoError("picture too wide".into()))?,
                height: u16::try_from(height).map_err(|_| VideoError("picture too tall".into()))?,
                pixels,
            });
        }
        Ok(last)
    }
}
