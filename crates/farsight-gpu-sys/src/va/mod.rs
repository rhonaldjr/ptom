//! H.264 encoding through VA-API (AMD and Intel GPUs), with libva loaded
//! at run time (ADR-0021). This is the thin, unsafe layer: it fills VA-API's
//! parameter buffers from plain Rust values and runs one frame at a time.
//! The stream's headers (SPS, PPS, slice headers) are written by the caller
//! in safe code and passed in as *packed headers*, which every driver
//! accepts and Mesa's AMD driver needs.
//!
//! Every value here is used from a single thread: VA-API handles aren't
//! thread-safe, and these types are deliberately not `Send`.

#[allow(
    unsafe_code,
    unsafe_op_in_unsafe_fn,
    missing_docs,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unnecessary_transmutes,
    clippy::all,
    clippy::pedantic,
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block
)]
mod bindings;

use std::ffi::{CStr, c_int, c_uint, c_void};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;

use bindings as sys;
use sys::{VABufferType, VAConfigAttribType, VAEntrypoint, VAProfile};

// bindgen names the bit-field unions inside the parameter buffers with a
// leading underscore; these names are what the code below uses.
#[allow(clippy::used_underscore_items)]
type SequenceFields = sys::_VAEncSequenceParameterBufferH264__bindgen_ty_1;
#[allow(clippy::used_underscore_items)]
type SequenceBits = sys::_VAEncSequenceParameterBufferH264__bindgen_ty_1__bindgen_ty_1;
#[allow(clippy::used_underscore_items)]
type PictureFields = sys::_VAEncPictureParameterBufferH264__bindgen_ty_1;
#[allow(clippy::used_underscore_items)]
type PictureBits = sys::_VAEncPictureParameterBufferH264__bindgen_ty_1__bindgen_ty_1;
#[allow(clippy::used_underscore_items)]
type RateControlFlags = sys::_VAEncMiscParameterRateControl__bindgen_ty_1;
#[allow(clippy::used_underscore_items)]
type RateControlBits = sys::_VAEncMiscParameterRateControl__bindgen_ty_1__bindgen_ty_1;

/// libva's DRM front end; it pulls in libva itself, so every symbol
/// resolves through this one library.
const LIBRARY: &str = "libva-drm.so.2";
/// VA-API's "no surface" id, used for unused reference slots.
const INVALID: u32 = sys::VA_INVALID_SURFACE;

/// A VA-API failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("VA-API: {0}")]
pub struct VaError(pub String);

/// Which encoder block of the GPU to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entrypoint {
    /// The fixed-function, low-power encoder (preferred).
    LowPower,
    /// The shader-assisted encoder.
    Full,
}

impl Entrypoint {
    fn raw(self) -> VAEntrypoint {
        match self {
            Self::LowPower => VAEntrypoint::VAEntrypointEncSliceLP,
            Self::Full => VAEntrypoint::VAEntrypointEncSlice,
        }
    }
}

/// An open VA-API device (a GPU render node).
pub struct Va {
    lib: sys::Libva,
    display: sys::VADisplay,
    /// Keeps the render node open as long as the display exists.
    _device: File,
}

impl Va {
    /// Loads libva and opens `render_node` (e.g. `/dev/dri/renderD128`).
    ///
    /// # Errors
    /// [`VaError`] when libva isn't installed or the device has no driver.
    pub fn open(render_node: &Path) -> Result<Self, VaError> {
        #[allow(unsafe_code)]
        // SAFETY: loading libva runs only its (and its dependencies')
        // initialisers, which have no preconditions; every symbol is
        // checked to exist (`--dynamic-link-require-all`).
        let lib = unsafe { sys::Libva::new(LIBRARY) }
            .map_err(|e| VaError(format!("can't load {LIBRARY} ({e}); is libva2 installed?")))?;
        let device = File::options()
            .read(true)
            .write(true)
            .open(render_node)
            .map_err(|e| VaError(format!("{}: {e}", render_node.display())))?;
        #[allow(unsafe_code)]
        // SAFETY: the descriptor is open and stays open as long as the
        // display (it's stored alongside it).
        let display = unsafe { lib.vaGetDisplayDRM(device.as_raw_fd()) };
        if display.is_null() {
            return Err(VaError("no VA-API display for this device".into()));
        }
        let va = Self {
            lib,
            display,
            _device: device,
        };
        let (mut major, mut minor) = (0, 0);
        #[allow(unsafe_code)]
        // SAFETY: `display` came from vaGetDisplayDRM; the out-pointers are
        // valid for the call.
        let status = unsafe {
            va.lib
                .vaInitialize(va.display, &raw mut major, &raw mut minor)
        };
        va.check(status, "initialise")?;
        Ok(va)
    }

    fn check(&self, status: sys::VAStatus, what: &str) -> Result<(), VaError> {
        // VA_STATUS_SUCCESS is 0.
        if status == 0 {
            return Ok(());
        }
        #[allow(unsafe_code)]
        // SAFETY: vaErrorStr returns a pointer to a static, NUL-terminated
        // string for any status (or null).
        let text = unsafe { self.lib.vaErrorStr(status) };
        let reason = if text.is_null() {
            format!("error {status}")
        } else {
            #[allow(unsafe_code)]
            // SAFETY: non-null, NUL-terminated and static (see above).
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        };
        Err(VaError(format!("{what}: {reason}")))
    }

    /// The best H.264 (Main profile) encoder entry point, if any.
    #[must_use]
    pub fn h264_encoder(&self) -> Option<Entrypoint> {
        let mut entrypoints = [VAEntrypoint::VAEntrypointVLD; 32];
        let mut count: c_int = 0;
        #[allow(unsafe_code)]
        // SAFETY: the array has room for 32 entries, more than VA-API
        // defines; `count` is written with how many were filled.
        let status = unsafe {
            self.lib.vaQueryConfigEntrypoints(
                self.display,
                VAProfile::VAProfileH264Main,
                entrypoints.as_mut_ptr(),
                &raw mut count,
            )
        };
        self.check(status, "query").ok()?;
        let found = entrypoints.get(..usize::try_from(count).ok()?)?;
        [Entrypoint::LowPower, Entrypoint::Full]
            .into_iter()
            .find(|e| found.contains(&e.raw()))
    }

    /// What the encoder supports for `attribute` (a bit mask), if anything.
    fn attribute(&self, entrypoint: Entrypoint, attribute: VAConfigAttribType) -> Option<u32> {
        let mut attrib = sys::VAConfigAttrib {
            type_: attribute,
            value: 0,
        };
        #[allow(unsafe_code)]
        // SAFETY: one attribute, passed by pointer for the call only.
        let status = unsafe {
            self.lib.vaGetConfigAttributes(
                self.display,
                VAProfile::VAProfileH264Main,
                entrypoint.raw(),
                &raw mut attrib,
                1,
            )
        };
        self.check(status, "attributes").ok()?;
        // VA_ATTRIB_NOT_SUPPORTED is 0x80000000.
        (attrib.value != 0x8000_0000).then_some(attrib.value)
    }
}

impl Drop for Va {
    fn drop(&mut self) {
        #[allow(unsafe_code)]
        // SAFETY: the display came from vaGetDisplayDRM in `open` (terminating
        // also frees a display whose vaInitialize failed); nothing uses it
        // after this (encoders own their `Va`, so they are gone first).
        unsafe {
            self.lib.vaTerminate(self.display);
        }
    }
}

/// How the stream is laid out, matching the SPS the caller writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sequence {
    /// Width in 16×16 macroblocks.
    pub width_mbs: u16,
    /// Height in macroblocks.
    pub height_mbs: u16,
    /// Pixels cropped off the right of the coded picture (even).
    pub crop_right: u16,
    /// Pixels cropped off the bottom (even).
    pub crop_bottom: u16,
    /// `level_idc` (e.g. 41 for level 4.1).
    pub level_idc: u8,
    /// Frames from one keyframe to the next.
    pub keyframe_interval: u32,
    /// `log2_max_frame_num_minus4`.
    pub log2_max_frame_num_minus4: u8,
    /// Target bit rate.
    pub bits_per_second: u32,
    /// Frames per second the rate control plans for.
    pub frame_rate: u32,
    /// The QP the first frame starts from (also the PPS's `pic_init_qp`).
    pub initial_qp: u8,
}

/// One frame's place in the stream, matching its slice header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// A keyframe (IDR) that starts a fresh stream.
    pub idr: bool,
    /// `frame_num` (0 at each keyframe).
    pub frame_num: u16,
    /// `idr_pic_id` of the last keyframe.
    pub idr_pic_id: u16,
}

/// A header the caller wrote, with emulation prevention already applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packed {
    /// The sequence parameter set (keyframes only).
    Sequence(Vec<u8>),
    /// The picture parameter set (keyframes only).
    Picture(Vec<u8>),
    /// The slice header and its exact length in bits (slice data follows
    /// mid-byte, so the last byte may be partly used).
    Slice(Vec<u8>, u32),
}

/// An H.264 encoding session on one GPU.
pub struct H264Encoder {
    va: Va,
    config: sys::VAConfigID,
    context: sys::VAContextID,
    /// Input surface, then two reconstructed frames (current and reference).
    surfaces: [sys::VASurfaceID; 3],
    coded: sys::VABufferID,
    /// The coded buffer's size: output beyond it is a driver bug, refused.
    coded_size: u32,
    image: sys::VAImage,
    sequence: Sequence,
    rate_control: u32,
    frames: u64,
}

/// Parameter buffers for one frame, destroyed after it's encoded.
struct Buffers<'a> {
    va: &'a Va,
    ids: Vec<sys::VABufferID>,
}

impl Buffers<'_> {
    fn add<T>(
        &mut self,
        context: sys::VAContextID,
        kind: VABufferType,
        value: &T,
    ) -> Result<(), VaError> {
        let size =
            c_uint::try_from(size_of::<T>()).map_err(|_| VaError("buffer too large".into()))?;
        let mut id = 0;
        #[allow(unsafe_code)]
        // SAFETY: `value` is a live `T` of `size` bytes, which libva copies
        // during the call (it never writes through the pointer).
        let status = unsafe {
            self.va.lib.vaCreateBuffer(
                self.va.display,
                context,
                kind,
                size,
                1,
                std::ptr::from_ref(value).cast_mut().cast::<c_void>(),
                &raw mut id,
            )
        };
        self.va.check(status, "buffer")?;
        self.ids.push(id);
        Ok(())
    }

    fn add_bytes(&mut self, context: sys::VAContextID, bytes: &[u8]) -> Result<(), VaError> {
        let size = c_uint::try_from(bytes.len()).map_err(|_| VaError("header too large".into()))?;
        let mut id = 0;
        #[allow(unsafe_code)]
        // SAFETY: as in `add`: libva copies `size` bytes from the slice.
        let status = unsafe {
            self.va.lib.vaCreateBuffer(
                self.va.display,
                context,
                VABufferType::VAEncPackedHeaderDataBufferType,
                size,
                1,
                bytes.as_ptr().cast_mut().cast::<c_void>(),
                &raw mut id,
            )
        };
        self.va.check(status, "header buffer")?;
        self.ids.push(id);
        Ok(())
    }
}

impl Drop for Buffers<'_> {
    fn drop(&mut self) {
        for &id in &self.ids {
            #[allow(unsafe_code)]
            // SAFETY: each id was created by vaCreateBuffer on this display
            // and is destroyed once.
            unsafe {
                self.va.lib.vaDestroyBuffer(self.va.display, id);
            }
        }
    }
}

/// `VAEncMiscParameterBuffer`: a type tag followed by its payload.
#[repr(C)]
struct Misc<T> {
    kind: sys::VAEncMiscParameterType,
    payload: T,
}

fn reference(surface: sys::VASurfaceID, frame_num: u16) -> sys::VAPictureH264 {
    let order = i32::from(frame_num) * 2;
    sys::VAPictureH264 {
        picture_id: surface,
        frame_idx: u32::from(frame_num),
        flags: sys::VA_PICTURE_H264_SHORT_TERM_REFERENCE,
        TopFieldOrderCnt: order,
        BottomFieldOrderCnt: order,
        ..sys::VAPictureH264::default()
    }
}

fn unused_reference() -> sys::VAPictureH264 {
    sys::VAPictureH264 {
        picture_id: INVALID,
        flags: sys::VA_PICTURE_H264_INVALID,
        ..sys::VAPictureH264::default()
    }
}

impl H264Encoder {
    /// Starts a session for `sequence` on `va`'s GPU.
    ///
    /// # Errors
    /// [`VaError`] if the GPU can't encode this stream (e.g. too large, or
    /// no packed-header support).
    pub fn new(va: Va, entrypoint: Entrypoint, sequence: Sequence) -> Result<Self, VaError> {
        let packed = va
            .attribute(
                entrypoint,
                VAConfigAttribType::VAConfigAttribEncPackedHeaders,
            )
            .unwrap_or(0);
        let wanted = sys::VA_ENC_PACKED_HEADER_SEQUENCE
            | sys::VA_ENC_PACKED_HEADER_PICTURE
            | sys::VA_ENC_PACKED_HEADER_SLICE;
        if packed & sys::VA_ENC_PACKED_HEADER_SLICE == 0 {
            return Err(VaError(
                "the driver doesn't take packed slice headers".into(),
            ));
        }
        let modes = va
            .attribute(entrypoint, VAConfigAttribType::VAConfigAttribRateControl)
            .unwrap_or(0);
        let rate_control = if modes & sys::VA_RC_CBR != 0 {
            sys::VA_RC_CBR
        } else {
            sys::VA_RC_CQP
        };
        let mut attribs = [
            sys::VAConfigAttrib {
                type_: VAConfigAttribType::VAConfigAttribRTFormat,
                value: sys::VA_RT_FORMAT_YUV420,
            },
            sys::VAConfigAttrib {
                type_: VAConfigAttribType::VAConfigAttribRateControl,
                value: rate_control,
            },
            sys::VAConfigAttrib {
                type_: VAConfigAttribType::VAConfigAttribEncPackedHeaders,
                value: packed & wanted,
            },
        ];
        let mut config = 0;
        #[allow(unsafe_code)]
        // SAFETY: three attributes, valid for the call; `config` is written.
        let status = unsafe {
            va.lib.vaCreateConfig(
                va.display,
                VAProfile::VAProfileH264Main,
                entrypoint.raw(),
                attribs.as_mut_ptr(),
                3,
                &raw mut config,
            )
        };
        va.check(status, "config")?;
        let width = u32::from(sequence.width_mbs) * 16;
        let height = u32::from(sequence.height_mbs) * 16;
        let mut encoder = Self {
            va,
            config,
            context: INVALID,
            surfaces: [INVALID; 3],
            coded: INVALID,
            coded_size: 0,
            image: sys::VAImage {
                image_id: INVALID,
                ..sys::VAImage::default()
            },
            sequence,
            rate_control,
            frames: 0,
        };
        // From here on, Drop releases whatever was created.
        encoder.create_surfaces(width, height)?;
        encoder.create_context(width, height)?;
        encoder.coded_size = width * height * 3 / 2 + 4096;
        encoder.create_coded_buffer(encoder.coded_size)?;
        encoder.create_image(width, height)?;
        Ok(encoder)
    }

    fn create_surfaces(&mut self, width: u32, height: u32) -> Result<(), VaError> {
        #[allow(unsafe_code)]
        // SAFETY: room for 3 surface ids; no attributes.
        let status = unsafe {
            self.va.lib.vaCreateSurfaces(
                self.va.display,
                sys::VA_RT_FORMAT_YUV420,
                width,
                height,
                self.surfaces.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                0,
            )
        };
        self.va.check(status, "surfaces")
    }

    fn create_context(&mut self, width: u32, height: u32) -> Result<(), VaError> {
        let (width, height) = (
            c_int::try_from(width).map_err(|_| VaError("too wide".into()))?,
            c_int::try_from(height).map_err(|_| VaError("too tall".into()))?,
        );
        #[allow(unsafe_code)]
        // SAFETY: the 3 surfaces exist; `context` is written.
        let status = unsafe {
            self.va.lib.vaCreateContext(
                self.va.display,
                self.config,
                width,
                height,
                // VA_PROGRESSIVE
                1,
                self.surfaces.as_mut_ptr(),
                3,
                &raw mut self.context,
            )
        };
        self.va.check(status, "context")
    }

    fn create_coded_buffer(&mut self, size: u32) -> Result<(), VaError> {
        #[allow(unsafe_code)]
        // SAFETY: no initial data; the driver allocates `size` bytes.
        let status = unsafe {
            self.va.lib.vaCreateBuffer(
                self.va.display,
                self.context,
                VABufferType::VAEncCodedBufferType,
                size,
                1,
                std::ptr::null_mut(),
                &raw mut self.coded,
            )
        };
        self.va.check(status, "output buffer")
    }

    fn create_image(&mut self, width: u32, height: u32) -> Result<(), VaError> {
        let mut format = sys::VAImageFormat {
            fourcc: sys::VA_FOURCC_NV12,
            // VA_LSB_FIRST
            byte_order: 1,
            bits_per_pixel: 12,
            ..sys::VAImageFormat::default()
        };
        let (width, height) = (
            c_int::try_from(width).map_err(|_| VaError("too wide".into()))?,
            c_int::try_from(height).map_err(|_| VaError("too tall".into()))?,
        );
        #[allow(unsafe_code)]
        // SAFETY: `format` and `image` are valid for the call.
        let status = unsafe {
            self.va.lib.vaCreateImage(
                self.va.display,
                &raw mut format,
                width,
                height,
                &raw mut self.image,
            )
        };
        self.va.check(status, "image")
    }

    /// Encodes one NV12 picture of the full coded size (`width_mbs × 16` by
    /// `height_mbs × 16`, rows packed) as `frame`, with the caller's headers.
    ///
    /// # Errors
    /// [`VaError`] when the picture has the wrong size or the GPU fails.
    pub fn encode(
        &mut self,
        nv12: &[u8],
        frame: Frame,
        headers: &[Packed],
    ) -> Result<Vec<u8>, VaError> {
        self.upload(nv12)?;
        let current = self.surfaces[1 + usize::from(!self.frames.is_multiple_of(2))];
        let previous = self.surfaces[1 + usize::from(self.frames.is_multiple_of(2))];
        let mut buffers = Buffers {
            va: &self.va,
            ids: Vec::new(),
        };
        let context = self.context;
        if frame.idr {
            buffers.add(
                context,
                VABufferType::VAEncSequenceParameterBufferType,
                &self.sequence_parameters(),
            )?;
            self.rate_control_parameters(&mut buffers)?;
        }
        buffers.add(
            context,
            VABufferType::VAEncPictureParameterBufferType,
            &self.picture_parameters(frame, current, previous),
        )?;
        for header in headers {
            let (kind, bytes, bits) = match header {
                Packed::Sequence(bytes) => (
                    sys::VAEncPackedHeaderType_VAEncPackedHeaderSequence,
                    bytes,
                    u32::try_from(bytes.len() * 8)
                        .map_err(|_| VaError("header too large".into()))?,
                ),
                Packed::Picture(bytes) => (
                    sys::VAEncPackedHeaderType_VAEncPackedHeaderPicture,
                    bytes,
                    u32::try_from(bytes.len() * 8)
                        .map_err(|_| VaError("header too large".into()))?,
                ),
                Packed::Slice(bytes, bits) => {
                    buffers.add(
                        context,
                        VABufferType::VAEncSliceParameterBufferType,
                        &self.slice_parameters(frame, previous),
                    )?;
                    (
                        sys::VAEncPackedHeaderType_VAEncPackedHeaderSlice,
                        bytes,
                        *bits,
                    )
                }
            };
            buffers.add(
                context,
                VABufferType::VAEncPackedHeaderParameterBufferType,
                &sys::VAEncPackedHeaderParameterBuffer {
                    type_: kind,
                    bit_length: bits,
                    has_emulation_bytes: 1,
                    ..sys::VAEncPackedHeaderParameterBuffer::default()
                },
            )?;
            buffers.add_bytes(context, bytes)?;
        }
        let input = self.surfaces[0];
        self.run(input, &mut buffers.ids)?;
        drop(buffers);
        let coded = self.read_output(input)?;
        // Only now is `current` the reference for the next frame.
        self.frames += 1;
        Ok(coded)
    }

    fn sequence_parameters(&self) -> sys::VAEncSequenceParameterBufferH264 {
        let s = self.sequence;
        let mut bits = SequenceBits::default();
        bits.set_chroma_format_idc(1);
        bits.set_frame_mbs_only_flag(1);
        bits.set_direct_8x8_inference_flag(1);
        bits.set_log2_max_frame_num_minus4(u32::from(s.log2_max_frame_num_minus4));
        bits.set_pic_order_cnt_type(2);
        let fields = SequenceFields { bits };
        sys::VAEncSequenceParameterBufferH264 {
            level_idc: s.level_idc,
            intra_period: s.keyframe_interval,
            intra_idr_period: s.keyframe_interval,
            ip_period: 1,
            bits_per_second: s.bits_per_second,
            max_num_ref_frames: 1,
            picture_width_in_mbs: s.width_mbs,
            picture_height_in_mbs: s.height_mbs,
            seq_fields: fields,
            frame_cropping_flag: u8::from(s.crop_right > 0 || s.crop_bottom > 0),
            // Offsets are in chroma samples: two pixels each for 4:2:0.
            frame_crop_right_offset: u32::from(s.crop_right / 2),
            frame_crop_bottom_offset: u32::from(s.crop_bottom / 2),
            ..sys::VAEncSequenceParameterBufferH264::default()
        }
    }

    fn rate_control_parameters(&self, buffers: &mut Buffers<'_>) -> Result<(), VaError> {
        let s = self.sequence;
        let mut flag_bits = RateControlBits::default();
        flag_bits.set_disable_frame_skip(1);
        let flags = RateControlFlags { bits: flag_bits };
        buffers.add(
            self.context,
            VABufferType::VAEncMiscParameterBufferType,
            &Misc {
                kind: sys::VAEncMiscParameterType::VAEncMiscParameterTypeRateControl,
                payload: sys::VAEncMiscParameterRateControl {
                    bits_per_second: s.bits_per_second,
                    target_percentage: 100,
                    window_size: 1000,
                    initial_qp: u32::from(s.initial_qp),
                    min_qp: 10,
                    max_qp: 42,
                    rc_flags: flags,
                    ..sys::VAEncMiscParameterRateControl::default()
                },
            },
        )?;
        buffers.add(
            self.context,
            VABufferType::VAEncMiscParameterBufferType,
            &Misc {
                kind: sys::VAEncMiscParameterType::VAEncMiscParameterTypeFrameRate,
                payload: sys::VAEncMiscParameterFrameRate {
                    framerate: s.frame_rate,
                    ..sys::VAEncMiscParameterFrameRate::default()
                },
            },
        )
    }

    fn picture_parameters(
        &self,
        frame: Frame,
        current: sys::VASurfaceID,
        previous: sys::VASurfaceID,
    ) -> sys::VAEncPictureParameterBufferH264 {
        let mut references = [unused_reference(); 16];
        if !frame.idr {
            references[0] = reference(previous, frame.frame_num.wrapping_sub(1));
        }
        let mut bits = PictureBits::default();
        bits.set_idr_pic_flag(u32::from(frame.idr));
        bits.set_reference_pic_flag(1);
        // CABAC (Main profile).
        bits.set_entropy_coding_mode_flag(1);
        bits.set_deblocking_filter_control_present_flag(1);
        let fields = PictureFields { bits };
        let mut current_picture = reference(current, frame.frame_num);
        current_picture.flags = 0;
        sys::VAEncPictureParameterBufferH264 {
            CurrPic: current_picture,
            ReferenceFrames: references,
            coded_buf: self.coded,
            frame_num: frame.frame_num,
            pic_init_qp: self.sequence.initial_qp,
            pic_fields: fields,
            ..sys::VAEncPictureParameterBufferH264::default()
        }
    }

    fn slice_parameters(
        &self,
        frame: Frame,
        previous: sys::VASurfaceID,
    ) -> sys::VAEncSliceParameterBufferH264 {
        let mut list0 = [unused_reference(); 32];
        if !frame.idr {
            list0[0] = reference(previous, frame.frame_num.wrapping_sub(1));
        }
        sys::VAEncSliceParameterBufferH264 {
            macroblock_address: 0,
            num_macroblocks: u32::from(self.sequence.width_mbs)
                * u32::from(self.sequence.height_mbs),
            macroblock_info: INVALID,
            // 2 = I, 0 = P.
            slice_type: if frame.idr { 2 } else { 0 },
            idr_pic_id: frame.idr_pic_id,
            RefPicList0: list0,
            RefPicList1: [unused_reference(); 32],
            ..sys::VAEncSliceParameterBufferH264::default()
        }
    }

    /// Copies the picture into the input surface.
    fn upload(&mut self, nv12: &[u8]) -> Result<(), VaError> {
        let width = usize::from(self.sequence.width_mbs) * 16;
        let height = usize::from(self.sequence.height_mbs) * 16;
        if nv12.len() != width * height * 3 / 2 {
            return Err(VaError("picture size doesn't match the stream".into()));
        }
        let mut mapped: *mut c_void = std::ptr::null_mut();
        #[allow(unsafe_code)]
        // SAFETY: the image's buffer exists; `mapped` is written.
        let status = unsafe {
            self.va
                .lib
                .vaMapBuffer(self.va.display, self.image.buf, &raw mut mapped)
        };
        self.va.check(status, "map image")?;
        let size = self.image.data_size as usize;
        let result = if mapped.is_null() {
            Err(VaError("image mapped to nothing".into()))
        } else {
            #[allow(unsafe_code)]
            // SAFETY: libva maps the image's whole buffer (`data_size`
            // bytes), writable, until vaUnmapBuffer below; nothing else
            // references it meanwhile.
            let destination = unsafe { std::slice::from_raw_parts_mut(mapped.cast::<u8>(), size) };
            copy_planes(&self.image, nv12, width, height, destination)
        };
        #[allow(unsafe_code)]
        // SAFETY: the buffer was mapped above; `destination` is out of scope.
        let status = unsafe { self.va.lib.vaUnmapBuffer(self.va.display, self.image.buf) };
        result?;
        self.va.check(status, "unmap image")?;
        let (w, h) = (
            c_uint::try_from(width).map_err(|_| VaError("too wide".into()))?,
            c_uint::try_from(height).map_err(|_| VaError("too tall".into()))?,
        );
        #[allow(unsafe_code)]
        // SAFETY: the image and the input surface exist and have this size.
        let status = unsafe {
            self.va.lib.vaPutImage(
                self.va.display,
                self.surfaces[0],
                self.image.image_id,
                0,
                0,
                w,
                h,
                0,
                0,
                w,
                h,
            )
        };
        self.va.check(status, "upload")
    }

    fn run(&self, input: sys::VASurfaceID, buffers: &mut [sys::VABufferID]) -> Result<(), VaError> {
        let count =
            c_int::try_from(buffers.len()).map_err(|_| VaError("too many buffers".into()))?;
        #[allow(unsafe_code)]
        // SAFETY: context and surface exist.
        let status = unsafe {
            self.va
                .lib
                .vaBeginPicture(self.va.display, self.context, input)
        };
        self.va.check(status, "begin")?;
        #[allow(unsafe_code)]
        // SAFETY: `buffers` holds `count` ids created for this context.
        let status = unsafe {
            self.va
                .lib
                .vaRenderPicture(self.va.display, self.context, buffers.as_mut_ptr(), count)
        };
        self.va.check(status, "render")?;
        #[allow(unsafe_code)]
        // SAFETY: a picture was begun on this context.
        let status = unsafe { self.va.lib.vaEndPicture(self.va.display, self.context) };
        self.va.check(status, "end")
    }

    /// Waits for the frame and copies the coded bytes out.
    fn read_output(&self, input: sys::VASurfaceID) -> Result<Vec<u8>, VaError> {
        #[allow(unsafe_code)]
        // SAFETY: the surface exists.
        let status = unsafe { self.va.lib.vaSyncSurface(self.va.display, input) };
        self.va.check(status, "wait")?;
        let mut mapped: *mut c_void = std::ptr::null_mut();
        #[allow(unsafe_code)]
        // SAFETY: the coded buffer exists; `mapped` is written.
        let status = unsafe {
            self.va
                .lib
                .vaMapBuffer(self.va.display, self.coded, &raw mut mapped)
        };
        self.va.check(status, "map output")?;
        let mut stream = Vec::new();
        let mut overflow = false;
        let mut segment = mapped.cast::<sys::VACodedBufferSegment>().cast_const();
        // A linked list of segments; bounded in case a driver loops it.
        for _ in 0..64 {
            if segment.is_null() {
                break;
            }
            #[allow(unsafe_code)]
            // SAFETY: non-null segments point to valid VACodedBufferSegment
            // structs inside the mapping, which lives until the unmap below.
            let current = unsafe { &*segment };
            if stream.len() + current.size as usize > self.coded_size as usize {
                stream.clear();
                overflow = true;
                break;
            }
            if !current.buf.is_null() {
                #[allow(unsafe_code)]
                // SAFETY: `buf` holds `size` coded bytes in the mapping.
                let bytes = unsafe {
                    std::slice::from_raw_parts(current.buf.cast::<u8>(), current.size as usize)
                };
                stream.extend_from_slice(bytes);
            }
            segment = current
                .next
                .cast::<sys::VACodedBufferSegment>()
                .cast_const();
        }
        #[allow(unsafe_code)]
        // SAFETY: mapped above; the slices borrowed from it are gone.
        let status = unsafe { self.va.lib.vaUnmapBuffer(self.va.display, self.coded) };
        self.va.check(status, "unmap output")?;
        if overflow {
            return Err(VaError(
                "the driver reported more output than its buffer holds".into(),
            ));
        }
        Ok(stream)
    }

    /// The rate-control mode in use (`VA_RC_CBR` or `VA_RC_CQP`).
    #[must_use]
    pub fn constant_bitrate(&self) -> bool {
        self.rate_control == sys::VA_RC_CBR
    }
}

/// Copies a packed NV12 picture into a mapped `VAImage` using its plane
/// offsets and pitches. Pure, so it's tested without a GPU.
fn copy_planes(
    image: &sys::VAImage,
    nv12: &[u8],
    width: usize,
    height: usize,
    destination: &mut [u8],
) -> Result<(), VaError> {
    let (luma, chroma) = nv12.split_at(width * height);
    let planes = [(luma, 0usize), (chroma, 1usize)];
    for (source, plane) in planes {
        let offset = image.offsets[plane] as usize;
        let pitch = image.pitches[plane] as usize;
        for (row, line) in source.chunks_exact(width).enumerate() {
            let start = offset + row * pitch;
            destination
                .get_mut(start..start + width)
                .ok_or_else(|| VaError("image is smaller than the picture".into()))?
                .copy_from_slice(line);
        }
    }
    Ok(())
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        let display = self.va.display;
        let lib = &self.va.lib;
        if self.image.image_id != INVALID {
            #[allow(unsafe_code)]
            // SAFETY: created in `create_image`, destroyed once.
            unsafe {
                lib.vaDestroyImage(display, self.image.image_id);
            }
        }
        if self.coded != INVALID {
            #[allow(unsafe_code)]
            // SAFETY: created in `create_coded_buffer`, destroyed once.
            unsafe {
                lib.vaDestroyBuffer(display, self.coded);
            }
        }
        if self.context != INVALID {
            #[allow(unsafe_code)]
            // SAFETY: created in `create_context`, destroyed once.
            unsafe {
                lib.vaDestroyContext(display, self.context);
            }
        }
        if self.surfaces[0] != INVALID {
            #[allow(unsafe_code)]
            // SAFETY: the 3 surfaces were created together, destroyed once.
            unsafe {
                lib.vaDestroySurfaces(display, self.surfaces.as_mut_ptr(), 3);
            }
        }
        #[allow(unsafe_code)]
        // SAFETY: created in `new`, destroyed once, after its context.
        unsafe {
            lib.vaDestroyConfig(display, self.config);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planes_land_at_the_images_offsets_and_pitches() {
        let image = sys::VAImage {
            pitches: [4, 4, 0],
            offsets: [0, 8, 0],
            ..sys::VAImage::default()
        };
        // 2×2 picture: 4 luma bytes, then 2 chroma bytes.
        let nv12 = [1, 2, 3, 4, 5, 6];
        let mut destination = [0u8; 12];
        copy_planes(&image, &nv12, 2, 2, &mut destination).unwrap();
        assert_eq!(destination, [1, 2, 0, 0, 3, 4, 0, 0, 5, 6, 0, 0]);
    }

    #[test]
    fn a_too_small_image_is_an_error_not_a_panic() {
        let image = sys::VAImage {
            pitches: [4, 4, 0],
            offsets: [0, 8, 0],
            ..sys::VAImage::default()
        };
        let mut destination = [0u8; 9];
        assert!(copy_planes(&image, &[0; 6], 2, 2, &mut destination).is_err());
    }

    #[test]
    fn a_missing_device_is_an_error() {
        assert!(Va::open(Path::new("/nonexistent/renderD999")).is_err());
    }
}
