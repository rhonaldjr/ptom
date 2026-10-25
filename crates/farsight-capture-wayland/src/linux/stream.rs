//! The PipeWire side: a dedicated OS thread runs the PipeWire main loop on the portal's fd and
//! turns buffers into [`CaptureEvent`]s. Nothing here ever blocks the loop: frames that cannot
//! be queued are dropped and their damage carried into the next delivered frame.

use std::cell::RefCell;
use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use farsight_codec::Framebuffer;
use pipewire as pw;
use pw::spa;
use pw::spa::buffer::meta::{MetaCursor, MetaVideoDamage};
use pw::spa::buffer::{ChunkFlags, DataType};
use pw::spa::param::ParamType;
use pw::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::{ChoiceValue, Object, Pod, Property, PropertyFlags, Value};
use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle, SpaTypes};
use pw::stream::{Stream, StreamFlags, StreamState};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use super::FrameSize;
use crate::damage::{self, Accumulator, Region};
use crate::pixels::{self, BYTES_PER_PIXEL, Layout, PixelFormat};
use crate::throttle::{MAX_FPS, Throttle};
use crate::{CaptureError, CaptureEvent, CursorShape, Frame};

/// Largest cursor image we ask for and accept.
const MAX_CURSOR: u32 = 256;
/// Largest frame we ask for (RFB sizes are 16-bit).
const MAX_FRAME: u32 = 16_384;
/// Most damage rectangles per buffer we ask for.
const DAMAGE_REGIONS: usize = 16;

/// The running PipeWire thread, as seen from the supervisor.
pub(super) struct PipeWire {
    /// Asks the thread to stop.
    pub quit: pw::channel::Sender<()>,
    /// Resolves when the thread has stopped, with the reason if it stopped on its own.
    pub done: oneshot::Receiver<Option<String>>,
}

/// Starts the PipeWire thread and waits until its stream is connected.
pub(super) async fn spawn(
    fd: OwnedFd,
    node_id: u32,
    events: mpsc::Sender<CaptureEvent>,
    frame_size: Arc<FrameSize>,
) -> Result<PipeWire, CaptureError> {
    let (ready_tx, ready_rx) = oneshot::channel();
    let (done_tx, done) = oneshot::channel();
    let (quit, quit_rx) = pw::channel::channel();
    std::thread::Builder::new()
        .name("farsight-pipewire".to_owned())
        .spawn(move || {
            let reason = Rc::new(RefCell::new(None));
            let thread = Thread {
                node_id,
                events,
                frame_size,
                reason: Rc::clone(&reason),
            };
            let mut ready = Some(ready_tx);
            if let Err(error) = thread.run(fd, quit_rx, &mut ready) {
                let message = format!("PipeWire error: {error}");
                match ready.take() {
                    Some(ready) => {
                        let _ = ready.send(Err(CaptureError::PipeWire(error.to_string())));
                    }
                    None => *reason.borrow_mut() = Some(message),
                }
            }
            let _ = done_tx.send(reason.borrow_mut().take());
        })
        .map_err(|error| CaptureError::PipeWire(error.to_string()))?;
    match ready_rx.await {
        Ok(Ok(())) => Ok(PipeWire { quit, done }),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(CaptureError::PipeWire(
            "the PipeWire thread stopped during setup".to_owned(),
        )),
    }
}

struct Thread {
    node_id: u32,
    events: mpsc::Sender<CaptureEvent>,
    frame_size: Arc<FrameSize>,
    reason: Rc<RefCell<Option<String>>>,
}

type Ready = Option<oneshot::Sender<Result<(), CaptureError>>>;

impl Thread {
    fn run(
        self,
        fd: OwnedFd,
        quit: pw::channel::Receiver<()>,
        ready: &mut Ready,
    ) -> Result<(), pw::Error> {
        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_fd_rc(fd, None)?;
        let stream = pw::stream::StreamBox::new(
            &core,
            "farsight-capture",
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )?;

        let capture = Rc::new(RefCell::new(Capture {
            events: self.events,
            frame_size: self.frame_size,
            format: None,
            throttle: Throttle::new(MAX_FPS),
            damage: Accumulator::default(),
            latest: None,
            cursor: CursorState::default(),
            reason: self.reason,
            mainloop: mainloop.downgrade(),
        }));
        // Callbacks run one at a time on this thread; `with` skips (rather than panics on) any
        // re-entrant call PipeWire might make from inside one of them.
        let _listener = stream
            .add_local_listener_with_user_data(Rc::clone(&capture))
            .state_changed(|_, capture, _, new| with(capture, |c| c.state_changed(new)))
            .param_changed(|stream, capture, id, param| {
                with(capture, |c| c.param_changed(stream, id, param));
            })
            .process(|stream, capture| with(capture, |c| c.process(stream)))
            .register()?;
        let timer_capture = Rc::clone(&capture);
        let flush_timer = mainloop
            .loop_()
            .add_timer(move |_| with(&timer_capture, Capture::flush));
        let interval = Throttle::interval(MAX_FPS);
        if let Err(error) = flush_timer
            .update_timer(Some(interval), Some(interval))
            .into_sync_result()
        {
            tracing::warn!("cannot start the frame flush timer: {error}");
        }

        let formats = enum_format_pods();
        let mut params: Vec<&Pod> = formats.iter().filter_map(|f| Pod::from_bytes(f)).collect();
        stream.connect(
            spa::utils::Direction::Input,
            Some(self.node_id),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )?;

        let weak = mainloop.downgrade();
        let _quit = quit.attach(mainloop.loop_(), move |()| {
            if let Some(mainloop) = weak.upgrade() {
                mainloop.quit();
            }
        });
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }
        mainloop.run();
        Ok(())
    }
}

/// The negotiated video format.
#[derive(Debug, Clone, Copy)]
struct VideoFormatInfo {
    format: PixelFormat,
    width: u32,
    height: u32,
    memory: Memory,
}

/// Which buffers the negotiated format allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Memory {
    /// Shared memory only (no modifier was negotiated).
    Shared,
    /// Also DMA-BUFs: the format fixed the modifier to LINEAR, so the CPU
    /// can read them row by row.
    #[cfg(feature = "dmabuf")]
    LinearDmaBuf,
}

/// The memory a negotiated format allows: DMA-BUF only when it carries a
/// modifier and that modifier is LINEAR (a tiled buffer read as rows would
/// be scrambled).
fn negotiated_memory(info: &VideoInfoRaw) -> Memory {
    #[cfg(feature = "dmabuf")]
    if info.flags().bits() & spa::sys::SPA_VIDEO_FLAG_MODIFIER != 0 && info.modifier() == 0 {
        return Memory::LinearDmaBuf;
    }
    let _ = info;
    Memory::Shared
}

/// A change to the pointer image.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CursorUpdate {
    Hidden,
    Shape(CursorShape),
}

impl CursorUpdate {
    fn event(&self) -> CaptureEvent {
        CaptureEvent::Cursor(match self {
            Self::Hidden => None,
            Self::Shape(shape) => Some(shape.clone()),
        })
    }
}

/// The last cursor we told the caller about, and one we still owe them.
#[derive(Debug, Default)]
struct CursorState {
    sent: Option<CursorUpdate>,
    pending: Option<CursorUpdate>,
}

/// Per-stream state, owned by the PipeWire listener.
struct Capture {
    events: mpsc::Sender<CaptureEvent>,
    frame_size: Arc<FrameSize>,
    format: Option<VideoFormatInfo>,
    throttle: Throttle,
    /// Damage since the last delivered frame.
    damage: Accumulator,
    /// The newest image not yet delivered.
    latest: Option<Framebuffer>,
    cursor: CursorState,
    reason: Rc<RefCell<Option<String>>>,
    mainloop: pw::main_loop::MainLoopWeak,
}

impl Capture {
    fn stop(&self, reason: Option<String>) {
        if let Some(reason) = reason {
            self.reason.borrow_mut().get_or_insert(reason);
        }
        if let Some(mainloop) = self.mainloop.upgrade() {
            mainloop.quit();
        }
    }

    fn state_changed(&mut self, state: StreamState) {
        match state {
            StreamState::Error(message) => {
                self.stop(Some(format!("the screen stream failed: {message}")));
            }
            StreamState::Unconnected => {
                self.stop(Some("the screen stream disconnected".to_owned()));
            }
            StreamState::Connecting | StreamState::Paused | StreamState::Streaming => {}
        }
    }

    fn param_changed(&mut self, stream: &Stream, id: u32, param: Option<&Pod>) {
        if id != ParamType::Format.as_raw() {
            return;
        }
        let Some(param) = param else { return };
        let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param) else {
            return;
        };
        if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
            return;
        }
        let mut info = VideoInfoRaw::default();
        if info.parse(param).is_err() {
            tracing::warn!("PipeWire sent a video format we could not parse");
            return;
        }
        let Some(format) = pixel_format(info.format()) else {
            tracing::warn!(
                "PipeWire chose an unsupported pixel format {:?}",
                info.format()
            );
            self.format = None;
            return;
        };
        let size = info.size();
        tracing::debug!(
            "PipeWire video format {:?} {}x{}",
            info.format(),
            size.width,
            size.height
        );
        let memory = negotiated_memory(&info);
        self.format = Some(VideoFormatInfo {
            format,
            width: size.width,
            height: size.height,
            memory,
        });
        self.frame_size.set(size.width, size.height);
        self.damage.reset();

        let pods: Vec<Vec<u8>> = buffer_param_objects(memory)
            .into_iter()
            .filter_map(serialize)
            .collect();
        let mut params: Vec<&Pod> = pods.iter().filter_map(|b| Pod::from_bytes(b)).collect();
        if let Err(error) = stream.update_params(&mut params) {
            tracing::warn!("PipeWire rejected our buffer parameters: {error}");
        }
    }

    /// Takes the newest buffer from PipeWire, keeps its image as the pending frame and delivers
    /// whatever the throttle and the queue allow.
    fn process(&mut self, stream: &Stream) {
        if let Some(buffer) = stream.dequeue_buffer() {
            self.take_buffer(buffer);
        }
        self.flush();
    }

    /// Delivers the pending cursor and frame if allowed; called per buffer and by a timer, so a
    /// throttled last frame still arrives once its interval is over.
    fn flush(&mut self) {
        if self.events.is_closed() {
            self.stop(None);
            return;
        }
        self.flush_cursor();
        let Some(framebuffer) = self.latest.take() else {
            return;
        };
        if self.events.capacity() == 0 || !self.throttle.admit(Instant::now()) {
            self.latest = Some(framebuffer);
            return;
        }
        let frame = Frame {
            framebuffer,
            damage: self.damage.take(),
        };
        if let Err(TrySendError::Full(CaptureEvent::Frame(frame))) =
            self.events.try_send(CaptureEvent::Frame(frame))
        {
            self.damage.add(frame.damage);
            self.latest = Some(frame.framebuffer);
        }
    }

    fn take_buffer(&mut self, mut buffer: pw::buffer::Buffer<'_>) {
        if let Some(cursor) = buffer.find_meta::<MetaCursor>().and_then(read_cursor)
            && self.cursor.sent.as_ref() != Some(&cursor)
        {
            self.cursor.pending = Some(cursor);
        }

        let Some(info) = self.format else { return };
        let (Ok(width), Ok(height)) = (u16::try_from(info.width), u16::try_from(info.height))
        else {
            return;
        };
        let damage = buffer
            .find_meta::<MetaVideoDamage>()
            .and_then(|meta| damage::convert(meta.iter().map(region_of), width, height));

        let Some(data) = buffer.datas_mut().first_mut() else {
            return;
        };
        let chunk = data.chunk();
        let (offset, size, stride) = (chunk.offset(), chunk.size(), chunk.stride());
        if size == 0 || chunk.flags().contains(ChunkFlags::CORRUPTED) {
            // A cursor-only update, or a frame the compositor marked as unusable.
            return;
        }
        let (Ok(offset), Ok(size)) = (usize::try_from(offset), usize::try_from(size)) else {
            return;
        };
        let stride = match usize::try_from(stride) {
            Ok(0) | Err(_) => usize::try_from(info.width).unwrap_or(0) * BYTES_PER_PIXEL,
            Ok(stride) => stride,
        };
        let layout = Layout {
            format: info.format,
            width: info.width,
            height: info.height,
            stride,
        };
        let convert = |bytes: &[u8]| match bytes.get(offset..offset.saturating_add(size)) {
            Some(image) => pixels::to_framebuffer(layout, image).map_err(|e| e.to_string()),
            None => Err("PipeWire chunk lies outside its buffer".to_owned()),
        };
        let converted = match data.type_() {
            DataType::MemPtr | DataType::MemFd => match data.data() {
                Some(bytes) => convert(bytes),
                None => return,
            },
            #[cfg(feature = "dmabuf")]
            DataType::DmaBuf if info.memory == Memory::LinearDmaBuf => read_dmabuf(data, convert),
            _ => {
                tracing::warn!("PipeWire sent a buffer we cannot map");
                return;
            }
        };
        match converted {
            Ok(framebuffer) => {
                self.damage.add(damage);
                self.latest = Some(framebuffer);
            }
            Err(error) => tracing::warn!("dropping a frame: {error}"),
        }
    }

    fn flush_cursor(&mut self) {
        let Some(cursor) = self.cursor.pending.take() else {
            return;
        };
        match self.events.try_send(cursor.event()) {
            Ok(()) => self.cursor.sent = Some(cursor),
            Err(_) => self.cursor.pending = Some(cursor),
        }
    }
}

/// Reads a DMA-BUF frame through a read-only mapping (`farsight-gpu-sys`,
/// ADR-0021). Its layout is linear: that's the only modifier we offer.
#[cfg(feature = "dmabuf")]
fn read_dmabuf(
    data: &spa::buffer::Data,
    convert: impl FnOnce(&[u8]) -> Result<Framebuffer, String>,
) -> Result<Framebuffer, String> {
    let raw = data.as_raw();
    let (Ok(fd), Ok(len)) = (i32::try_from(raw.fd), usize::try_from(raw.maxsize)) else {
        return Err("PipeWire sent an unusable DMA-BUF".to_owned());
    };
    farsight_gpu_sys::read_dmabuf(fd, u64::from(raw.mapoffset), len, convert)
        .map_err(|e| format!("reading the DMA-BUF: {e}"))?
}

fn with(capture: &Rc<RefCell<Capture>>, f: impl FnOnce(&mut Capture)) {
    if let Ok(mut capture) = capture.try_borrow_mut() {
        f(&mut capture);
    }
}

fn pixel_format(format: VideoFormat) -> Option<PixelFormat> {
    [
        (VideoFormat::BGRx, PixelFormat::Bgrx),
        (VideoFormat::RGBx, PixelFormat::Rgbx),
        (VideoFormat::BGRA, PixelFormat::Bgra),
        (VideoFormat::RGBA, PixelFormat::Rgba),
    ]
    .into_iter()
    .find_map(|(spa, ours)| (spa == format).then_some(ours))
}

fn region_of(meta: &spa::buffer::meta::MetaRegion) -> Region {
    let region = meta.region();
    Region {
        x: region.position.x,
        y: region.position.y,
        width: region.size.width,
        height: region.size.height,
    }
}

/// The cursor change in this metadata, or `None` if there is nothing new. Position-only
/// updates are ignored: the viewer draws the pointer where its own mouse is.
fn read_cursor(meta: &MetaCursor) -> Option<CursorUpdate> {
    if !meta.is_valid() {
        return None;
    }
    let bitmap = meta.bitmap()?;
    let size = bitmap.size();
    if size.width == 0 || size.height == 0 {
        return Some(CursorUpdate::Hidden);
    }
    if size.width > MAX_CURSOR || size.height > MAX_CURSOR {
        return None;
    }
    let format = pixel_format(bitmap.format())?;
    let layout = Layout {
        format,
        width: size.width,
        height: size.height,
        stride: usize::try_from(bitmap.stride()).ok()?,
    };
    let rgba = pixels::to_rgba(layout, bitmap.bitmap_data()?).ok()?;
    let hotspot = meta.hotspot();
    let clamp = |v: i32, limit: u32| {
        u16::try_from(v.clamp(0, i32::try_from(limit).unwrap_or(0) - 1)).unwrap_or(0)
    };
    Some(CursorUpdate::Shape(CursorShape {
        width: u16::try_from(size.width).ok()?,
        height: u16::try_from(size.height).ok()?,
        hotspot: (clamp(hotspot.x, size.width), clamp(hotspot.y, size.height)),
        rgba,
    }))
}

fn serialize(object: Object) -> Option<Vec<u8>> {
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .ok()
        .map(|(cursor, _)| cursor.into_inner())
}

fn property(key: u32, value: Value) -> Property {
    Property {
        key,
        flags: PropertyFlags::empty(),
        value,
    }
}

fn int_range(default: usize, min: usize, max: usize) -> Value {
    let int = |v: usize| i32::try_from(v).unwrap_or(i32::MAX);
    Value::Choice(ChoiceValue::Int(Choice(
        ChoiceFlags::empty(),
        ChoiceEnum::Range {
            default: int(default),
            min: int(min),
            max: int(max),
        },
    )))
}

/// The formats we accept, preferred first: with the `dmabuf` feature, a
/// linear DMA-BUF (the compositor then skips its copy to shared memory);
/// always, 32-bit RGB in shared memory (no modifier, so no DMA-BUF).
fn enum_format_pods() -> Vec<Vec<u8>> {
    let mut formats = Vec::new();
    #[cfg(feature = "dmabuf")]
    formats.extend(serialize(linear_dmabuf_format()));
    formats.extend(serialize(format_object()));
    formats
}

/// The shared-memory format plus `modifier = LINEAR`, mandatory: buffers
/// the CPU can read row by row.
#[cfg(feature = "dmabuf")]
fn linear_dmabuf_format() -> Object {
    /// `DRM_FORMAT_MOD_LINEAR`.
    const LINEAR: i64 = 0;
    let mut object = format_object();
    object.properties.push(Property {
        key: FormatProperties::VideoModifier.as_raw(),
        // One choice, so nothing for us to fixate (no DONT_FIXATE).
        flags: PropertyFlags::MANDATORY,
        value: Value::Choice(ChoiceValue::Long(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Enum {
                default: LINEAR,
                alternatives: vec![LINEAR],
            },
        ))),
    });
    object
}

/// 32-bit RGB video of any size up to [`MAX_FRAME`].
fn format_object() -> Object {
    let video_formats = [
        VideoFormat::BGRx,
        VideoFormat::RGBx,
        VideoFormat::BGRA,
        VideoFormat::RGBA,
    ]
    .map(|f| Id(f.as_raw()));
    let rectangle = |width, height| Rectangle { width, height };
    Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: vec![
            property(
                FormatProperties::MediaType.as_raw(),
                Value::Id(Id(MediaType::Video.as_raw())),
            ),
            property(
                FormatProperties::MediaSubtype.as_raw(),
                Value::Id(Id(MediaSubtype::Raw.as_raw())),
            ),
            property(
                FormatProperties::VideoFormat.as_raw(),
                Value::Choice(ChoiceValue::Id(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum {
                        default: video_formats[0],
                        alternatives: video_formats.to_vec(),
                    },
                ))),
            ),
            property(
                FormatProperties::VideoSize.as_raw(),
                Value::Choice(ChoiceValue::Rectangle(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: rectangle(1920, 1080),
                        min: rectangle(1, 1),
                        max: rectangle(MAX_FRAME, MAX_FRAME),
                    },
                ))),
            ),
            property(
                FormatProperties::VideoFramerate.as_raw(),
                Value::Choice(ChoiceValue::Fraction(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: Fraction {
                            num: MAX_FPS,
                            denom: 1,
                        },
                        min: Fraction { num: 0, denom: 1 },
                        max: Fraction {
                            num: 1000,
                            denom: 1,
                        },
                    },
                ))),
            ),
        ],
    }
}

/// Size of a cursor metadata block holding a `side` × `side` RGBA bitmap.
fn cursor_meta_size(side: u32) -> usize {
    let side = usize::try_from(side).unwrap_or(0);
    std::mem::size_of::<spa::sys::spa_meta_cursor>()
        + std::mem::size_of::<spa::sys::spa_meta_bitmap>()
        + side * side * BYTES_PER_PIXEL
}

/// Buffers in shared memory (plus DMA-BUFs when `memory` allows them),
/// with damage and cursor metadata.
fn buffer_param_objects(memory: Memory) -> Vec<Object> {
    let shared = (1u32 << spa::sys::SPA_DATA_MemPtr) | (1u32 << spa::sys::SPA_DATA_MemFd);
    let shm_types = match memory {
        Memory::Shared => shared,
        #[cfg(feature = "dmabuf")]
        Memory::LinearDmaBuf => shared | (1u32 << spa::sys::SPA_DATA_DmaBuf),
    };
    let region = std::mem::size_of::<spa::sys::spa_meta_region>();
    let meta = |kind: u32, size: Value| Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: ParamType::Meta.as_raw(),
        properties: vec![
            property(spa::sys::SPA_PARAM_META_type, Value::Id(Id(kind))),
            property(spa::sys::SPA_PARAM_META_size, size),
        ],
    };
    vec![
        Object {
            type_: SpaTypes::ObjectParamBuffers.as_raw(),
            id: ParamType::Buffers.as_raw(),
            properties: vec![property(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: i32::try_from(shm_types).unwrap_or(0),
                        flags: Vec::new(),
                    },
                ))),
            )],
        },
        meta(
            spa::sys::SPA_META_VideoDamage,
            int_range(region * DAMAGE_REGIONS, region, region * DAMAGE_REGIONS),
        ),
        meta(
            spa::sys::SPA_META_Cursor,
            int_range(
                cursor_meta_size(64),
                cursor_meta_size(1),
                cursor_meta_size(MAX_CURSOR),
            ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_serialize_to_valid_pods() {
        let formats = enum_format_pods();
        // One format, or (with `dmabuf`) the linear DMA-BUF one first.
        assert_eq!(formats.len(), if cfg!(feature = "dmabuf") { 2 } else { 1 });
        for format in &formats {
            assert!(Pod::from_bytes(format).is_some());
        }
        for object in buffer_param_objects(Memory::Shared) {
            let bytes = serialize(object).unwrap();
            assert!(Pod::from_bytes(&bytes).is_some());
        }
    }

    #[test]
    fn maps_only_the_negotiated_formats() {
        assert_eq!(pixel_format(VideoFormat::BGRx), Some(PixelFormat::Bgrx));
        assert_eq!(pixel_format(VideoFormat::RGBA), Some(PixelFormat::Rgba));
        assert_eq!(pixel_format(VideoFormat::I420), None);
    }

    #[test]
    fn cursor_meta_fits_a_full_bitmap() {
        assert!(cursor_meta_size(64) >= 64 * 64 * 4);
        assert!(cursor_meta_size(1) < cursor_meta_size(MAX_CURSOR));
    }
}
