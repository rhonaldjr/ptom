//! Video codecs (Open H.264, rfbproto encoding 50) as ports: the protocol
//! code here frames and routes the data; an adapter (`farsight-h264`)
//! supplies the actual codec, so this crate stays free of native code.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use farsight_rfb::wire::Reader;
use farsight_rfb::{DecodeError, Rect};

use crate::Framebuffer;
use crate::util::need;

/// `ResetContext` flag: forget the decoder for this rectangle first.
pub const RESET_CONTEXT: u32 = 1;
/// `ResetAllContexts` flag: forget every decoder first.
pub const RESET_ALL_CONTEXTS: u32 = 2;
/// Largest H.264 payload accepted in one rectangle.
pub const MAX_VIDEO_PAYLOAD: usize = 16 << 20;
/// Most decoder contexts kept at once (each holds reference frames).
pub const MAX_CONTEXTS: usize = 8;
/// Most pictures one rectangle may carry (an honest server sends one).
pub const MAX_PICTURES_PER_RECT: usize = 4;

/// `size` rounded up to even, as H.264 pictures are.
#[must_use]
pub fn coded_size((width, height): (u16, u16)) -> (usize, usize) {
    (
        usize::from(width).next_multiple_of(2),
        usize::from(height).next_multiple_of(2),
    )
}

/// A codec failure (the codec's own message).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("video codec: {0}")]
pub struct VideoError(pub String);

/// A decoded picture: `width` × `height` pixels as `0x00RRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// Width (may be the rectangle's, rounded up to even).
    pub width: u16,
    /// Height (likewise).
    pub height: u16,
    /// Row-major pixels.
    pub pixels: Vec<u32>,
}

/// Decodes one stream of H.264 frames (one rectangle's context).
pub trait VideoDecoder: Send {
    /// Decodes `data` (whole frames, at most [`MAX_PICTURES_PER_RECT`]);
    /// returns the last picture. Every picture must be `size` rounded up to
    /// even; anything else is refused before its pixels are converted, so a
    /// hostile stream can't make the viewer allocate a huge picture.
    ///
    /// # Errors
    /// [`VideoError`] if the stream is invalid or a picture has another size.
    fn decode(&mut self, data: &[u8], size: (u16, u16)) -> Result<Option<Picture>, VideoError>;
}

/// Encodes one stream of frames of a fixed size.
pub trait VideoEncoder: Send {
    /// Encodes `pixels` (`0x00RRGGBB`, the encoder's width × height).
    ///
    /// # Errors
    /// [`VideoError`] if the codec fails.
    fn encode(&mut self, pixels: &[u32]) -> Result<Vec<u8>, VideoError>;
}

/// Makes codecs. Absent when no H.264 library is installed or enabled.
pub trait VideoCodecs: Send + Sync {
    /// A new decoder.
    ///
    /// # Errors
    /// [`VideoError`] if the codec can't be created.
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, VideoError>;

    /// A new encoder for `width` × `height` frames (both even).
    ///
    /// # Errors
    /// [`VideoError`] if the codec can't be created.
    fn encoder(&self, width: u16, height: u16) -> Result<Box<dyn VideoEncoder>, VideoError>;
}

/// Codecs that hand out at most `max` encoders at a time: each is a GPU
/// session or a busy CPU core, so a crowd of viewers can't exhaust the agent.
/// Viewers beyond the limit get the lossless encodings instead.
pub struct LimitedCodecs {
    inner: Arc<dyn VideoCodecs>,
    live: Arc<AtomicUsize>,
    max: usize,
}

impl LimitedCodecs {
    /// Wraps `inner`, allowing `max` encoders at once.
    #[must_use]
    pub fn new(inner: Arc<dyn VideoCodecs>, max: usize) -> Self {
        Self {
            inner,
            live: Arc::new(AtomicUsize::new(0)),
            max,
        }
    }
}

impl VideoCodecs for LimitedCodecs {
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, VideoError> {
        self.inner.decoder()
    }

    fn encoder(&self, width: u16, height: u16) -> Result<Box<dyn VideoEncoder>, VideoError> {
        // Claim a slot; give it back at once if that went over the limit.
        let slot = Slot(Arc::clone(&self.live));
        if slot.0.fetch_add(1, Ordering::AcqRel) >= self.max {
            drop(slot);
            return Err(VideoError(format!("already {} H.264 streams", self.max)));
        }
        let encoder = self.inner.encoder(width, height)?;
        Ok(Box::new(Counted {
            encoder,
            _slot: slot,
        }))
    }
}

/// One claimed stream; released when dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Counted {
    encoder: Box<dyn VideoEncoder>,
    _slot: Slot,
}

impl VideoEncoder for Counted {
    fn encode(&mut self, pixels: &[u32]) -> Result<Vec<u8>, VideoError> {
        self.encoder.encode(pixels)
    }
}

/// Shared codecs that can sit in `Debug` configuration structs.
#[derive(Clone)]
pub struct SharedCodecs(pub Arc<dyn VideoCodecs>);

/// Equal when they are the same codecs (pointer identity), so connection
/// requests that carry them can still be compared and hashed.
impl PartialEq for SharedCodecs {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SharedCodecs {}

impl std::hash::Hash for SharedCodecs {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).cast::<()>().hash(state);
    }
}

impl std::fmt::Debug for SharedCodecs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedCodecs")
    }
}

/// The viewer's side: one decoder per rectangle context.
pub(crate) struct VideoContexts {
    codecs: Arc<dyn VideoCodecs>,
    contexts: HashMap<Rect, Box<dyn VideoDecoder>>,
}

impl std::fmt::Debug for VideoContexts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoContexts")
            .field("contexts", &self.contexts.len())
            .finish_non_exhaustive()
    }
}

impl VideoContexts {
    pub(crate) fn new(codecs: Arc<dyn VideoCodecs>) -> Self {
        Self {
            codecs,
            contexts: HashMap::new(),
        }
    }

    /// Decodes an Open H.264 rectangle into `fb` (waits for the whole payload).
    pub(crate) fn decode(
        &mut self,
        fb: &mut Framebuffer,
        rect: Rect,
        input: &[u8],
    ) -> Result<Option<usize>, DecodeError> {
        fb.check(rect)?;
        let mut r = Reader::new(input);
        let len = usize::try_from(need!(r.u32())).unwrap_or(usize::MAX);
        let flags = need!(r.u32());
        if len > MAX_VIDEO_PAYLOAD {
            return Err(DecodeError::LimitExceeded {
                what: "H.264 rectangle",
                len: len as u64,
                max: MAX_VIDEO_PAYLOAD as u64,
            });
        }
        let data = need!(r.bytes(len));
        if flags & RESET_ALL_CONTEXTS != 0 {
            self.contexts.clear();
        }
        if flags & RESET_CONTEXT != 0 {
            self.contexts.remove(&rect);
        }
        if data.is_empty() {
            return Ok(Some(r.consumed()));
        }
        if !self.contexts.contains_key(&rect) && self.contexts.len() >= MAX_CONTEXTS {
            return Err(DecodeError::Malformed("too many H.264 contexts"));
        }
        let decoder = match self.contexts.entry(rect) {
            std::collections::hash_map::Entry::Occupied(slot) => slot.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => slot.insert(
                self.codecs
                    .decoder()
                    .map_err(|_| DecodeError::Malformed("no H.264 decoder"))?,
            ),
        };
        let picture = decoder
            .decode(data, (rect.width, rect.height))
            .map_err(|_| DecodeError::Malformed("invalid H.264 data"))?;
        if let Some(picture) = picture {
            // The picture may be padded to even sizes; it must cover the rectangle.
            if picture.width < rect.width || picture.height < rect.height {
                return Err(DecodeError::Malformed(
                    "H.264 picture smaller than its rectangle",
                ));
            }
            let stride = usize::from(picture.width);
            let width = usize::from(rect.width);
            fb.write(rect, |i| {
                let (x, y) = (i % width, i / width);
                picture.pixels.get(y * stride + x).copied().unwrap_or(0)
            })?;
        }
        Ok(Some(r.consumed()))
    }
}

/// The payload of an Open H.264 rectangle: length, flags, data.
#[must_use]
pub fn open_h264_payload(flags: u32, data: &[u8]) -> Vec<u8> {
    use farsight_rfb::wire::WriteExt;
    let mut out = Vec::with_capacity(8 + data.len());
    out.put_u32(u32::try_from(data.len()).unwrap_or(u32::MAX));
    out.put_u32(flags);
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Null;
    struct NullEncoder;

    impl VideoEncoder for NullEncoder {
        fn encode(&mut self, _: &[u32]) -> Result<Vec<u8>, VideoError> {
            Ok(Vec::new())
        }
    }

    impl VideoCodecs for Null {
        fn decoder(&self) -> Result<Box<dyn VideoDecoder>, VideoError> {
            Err(VideoError("none".into()))
        }
        fn encoder(&self, _: u16, _: u16) -> Result<Box<dyn VideoEncoder>, VideoError> {
            Ok(Box::new(NullEncoder))
        }
    }

    #[test]
    fn at_most_max_streams_and_slots_come_back() {
        let codecs = LimitedCodecs::new(Arc::new(Null), 2);
        let first = codecs.encoder(2, 2).unwrap();
        let _second = codecs.encoder(2, 2).unwrap();
        assert!(
            codecs.encoder(2, 2).is_err(),
            "a third viewer stays lossless"
        );
        drop(first);
        assert!(
            codecs.encoder(2, 2).is_ok(),
            "a closed stream frees its slot"
        );
    }

    #[test]
    fn coded_sizes_round_up_to_even() {
        assert_eq!(coded_size((639, 359)), (640, 360));
        assert_eq!(coded_size((640, 360)), (640, 360));
    }
}
