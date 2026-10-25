//! H.264 encoding on NVIDIA GPUs through NVENC, with NVIDIA's libraries
//! (`libcuda.so.1`, `libnvidia-encode.so.1`, both part of the driver) loaded
//! at run time (ADR-0021). Frames go in as NV12 through NVENC's own input
//! buffers; NVENC writes the whole stream, headers included. API 12.0, so
//! NVIDIA driver 520 or newer.
//!
//! Every value here is used from a single thread (the CUDA context is
//! current on the thread that made it); these types are deliberately not
//! `Send`.

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

use std::ffi::c_void;

use bindings as sys;

const CUDA_LIBRARY: &str = "libcuda.so.1";
const NVENC_LIBRARY: &str = "libnvidia-encode.so.1";

/// `NVENCAPI_VERSION` for the 12.0 headers: major | minor << 24.
const API_VERSION: u32 = sys::NVENCAPI_MAJOR_VERSION | (sys::NVENCAPI_MINOR_VERSION << 24);

/// `NVENCAPI_STRUCT_VERSION(ver)`.
const fn struct_version(version: u32) -> u32 {
    API_VERSION | (version << 16) | (0x7 << 28)
}

// Struct versions from nvEncodeAPI.h 12.0 (some carry bit 31).
const OPEN_SESSION_VER: u32 = struct_version(1);
const FUNCTION_LIST_VER: u32 = struct_version(2);
const CONFIG_VER: u32 = struct_version(8) | (1 << 31);
const PRESET_CONFIG_VER: u32 = struct_version(4) | (1 << 31);
const INITIALIZE_PARAMS_VER: u32 = struct_version(5) | (1 << 31);
const RC_PARAMS_VER: u32 = struct_version(1);
const CREATE_INPUT_BUFFER_VER: u32 = struct_version(1);
const CREATE_BITSTREAM_BUFFER_VER: u32 = struct_version(1);
const LOCK_INPUT_BUFFER_VER: u32 = struct_version(1);
const PIC_PARAMS_VER: u32 = struct_version(6) | (1 << 31);
const LOCK_BITSTREAM_VER: u32 = struct_version(2);

/// `NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS`.
const FORCE_KEYFRAME: u32 = 0x2 | 0x4;

// bindgen gives C enums typedef'd from `_NAME` underscore-prefixed names.
#[allow(clippy::used_underscore_items)]
mod consts {
    use super::sys;
    pub const SUCCESS: sys::NVENCSTATUS = sys::_NVENCSTATUS_NV_ENC_SUCCESS;
    pub const DEVICE_CUDA: sys::NV_ENC_DEVICE_TYPE =
        sys::_NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_CUDA;
    pub const NV12: sys::NV_ENC_BUFFER_FORMAT =
        sys::_NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_NV12;
    pub const CBR: sys::NV_ENC_PARAMS_RC_MODE = sys::_NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR;
    pub const FRAME: sys::NV_ENC_PIC_STRUCT = sys::_NV_ENC_PIC_STRUCT_NV_ENC_PIC_STRUCT_FRAME;
    pub const CABAC: sys::NV_ENC_H264_ENTROPY_CODING_MODE =
        sys::_NV_ENC_H264_ENTROPY_CODING_MODE_NV_ENC_H264_ENTROPY_CODING_MODE_CABAC;
}

const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> sys::GUID {
    sys::GUID {
        Data1: data1,
        Data2: data2,
        Data3: data3,
        Data4: data4,
    }
}

// GUIDs from nvEncodeAPI.h.
const CODEC_H264: sys::GUID = guid(
    0x6bc8_2762,
    0x4e63,
    0x4ca4,
    [0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf],
);
const PROFILE_MAIN: sys::GUID = guid(
    0x60b5_c1d4,
    0x67fe,
    0x4790,
    [0x94, 0xd5, 0xc4, 0x72, 0x6d, 0x7b, 0x6e, 0x6d],
);
const PRESET_P4: sys::GUID = guid(
    0x90a7_b826,
    0xdf06,
    0x4862,
    [0xb9, 0xd2, 0xcd, 0x6d, 0x73, 0xa0, 0x86, 0x81],
);

/// An NVENC or CUDA failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("NVENC: {0}")]
pub struct NvencError(pub String);

fn check(status: sys::NVENCSTATUS, what: &str) -> Result<(), NvencError> {
    if status == consts::SUCCESS {
        Ok(())
    } else {
        Err(NvencError(format!("{what} failed (status {status})")))
    }
}

/// What the stream should look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Width in pixels (even).
    pub width: u32,
    /// Height in pixels (even).
    pub height: u32,
    /// Target bit rate.
    pub bits_per_second: u32,
    /// Frames per second the rate control plans for.
    pub frame_rate: u32,
    /// Frames from one keyframe to the next.
    pub keyframe_interval: u32,
}

/// A CUDA context on the first NVIDIA GPU.
struct Cuda {
    destroy: sys::tcuCtxDestroy_v2,
    context: sys::CUcontext,
    /// Keeps libcuda loaded while the function pointers are in use.
    _library: libloading::Library,
}

impl Cuda {
    fn open() -> Result<Self, NvencError> {
        let unavailable =
            |e: libloading::Error| NvencError(format!("no NVIDIA driver ({CUDA_LIBRARY}: {e})"));
        #[allow(unsafe_code)]
        // SAFETY: loading the NVIDIA driver library runs only its own
        // initialisers, which have no preconditions.
        let library = unsafe { libloading::Library::new(CUDA_LIBRARY) }.map_err(unavailable)?;
        #[allow(unsafe_code)]
        // SAFETY: the type matches cuInit's C declaration (dynlink_cuda.h).
        let init = unsafe { library.get::<sys::tcuInit>(b"cuInit\0") }.map_err(unavailable)?;
        #[allow(unsafe_code)]
        // SAFETY: as above, for cuDeviceGet.
        let device_get =
            unsafe { library.get::<sys::tcuDeviceGet>(b"cuDeviceGet\0") }.map_err(unavailable)?;
        #[allow(unsafe_code)]
        // SAFETY: as above, for cuCtxCreate_v2.
        let create = unsafe { library.get::<sys::tcuCtxCreate_v2>(b"cuCtxCreate_v2\0") }
            .map_err(unavailable)?;
        #[allow(unsafe_code)]
        // SAFETY: as above, for cuCtxDestroy_v2.
        let destroy = unsafe { library.get::<sys::tcuCtxDestroy_v2>(b"cuCtxDestroy_v2\0") }
            .map_err(unavailable)?;
        let (init, device_get, create, destroy) = (*init, *device_get, *create, *destroy);
        let missing = || NvencError("the NVIDIA driver lacks a CUDA function".into());
        let (Some(init_fn), Some(device_get_fn), Some(create_fn)) = (init, device_get, create)
        else {
            return Err(missing());
        };
        destroy.ok_or_else(missing)?;
        #[allow(unsafe_code)]
        // SAFETY: cuInit(0) has no other preconditions.
        let status = unsafe { init_fn(0) };
        cuda_check(status, "cuInit")?;
        let mut device = 0;
        #[allow(unsafe_code)]
        // SAFETY: `device` is valid for the call; ordinal 0 is the first GPU.
        let status = unsafe { device_get_fn(&raw mut device, 0) };
        cuda_check(status, "cuDeviceGet")?;
        let mut context = std::ptr::null_mut();
        #[allow(unsafe_code)]
        // SAFETY: `context` is written with a new context, current on this
        // thread, which `Drop` destroys.
        let status = unsafe { create_fn(&raw mut context, 0, device) };
        cuda_check(status, "cuCtxCreate")?;
        Ok(Self {
            destroy,
            context,
            _library: library,
        })
    }
}

fn cuda_check(status: sys::CUresult, what: &str) -> Result<(), NvencError> {
    if status == 0 {
        Ok(())
    } else {
        Err(NvencError(format!("{what} failed (CUDA error {status})")))
    }
}

impl Drop for Cuda {
    fn drop(&mut self) {
        if let Some(destroy) = self.destroy {
            #[allow(unsafe_code)]
            // SAFETY: the context was created in `open` and is destroyed once,
            // after the encoder that used it (field order in `NvencEncoder`).
            unsafe {
                destroy(self.context);
            }
        }
    }
}

/// An NVENC H.264 encoding session.
pub struct NvencEncoder {
    api: sys::NV_ENCODE_API_FUNCTION_LIST,
    encoder: *mut c_void,
    input: sys::NV_ENC_INPUT_PTR,
    output: sys::NV_ENC_OUTPUT_PTR,
    settings: Settings,
    frames: u32,
    // Dropped after the encoder (declaration order): the library, then CUDA.
    _library: sys::NvencLibrary,
    cuda: Cuda,
}

impl NvencEncoder {
    /// Opens the first NVIDIA GPU and starts a session.
    ///
    /// # Errors
    /// [`NvencError`] without an NVIDIA driver, with a driver older than
    /// 520, or when the GPU can't encode these settings.
    pub fn new(settings: Settings) -> Result<Self, NvencError> {
        let mut encoder = Self::open(settings)?;
        encoder.initialise()?;
        encoder.create_buffers()?;
        Ok(encoder)
    }

    /// Loads the driver and opens an encode session (nothing allocated yet;
    /// `Drop` closes it).
    fn open(settings: Settings) -> Result<Self, NvencError> {
        let cuda = Cuda::open()?;
        #[allow(unsafe_code)]
        // SAFETY: loading the driver's encoder library runs only its own
        // initialisers; both symbols are checked to exist.
        let library = unsafe { sys::NvencLibrary::new(NVENC_LIBRARY) }
            .map_err(|e| NvencError(format!("no NVENC ({NVENC_LIBRARY}: {e})")))?;
        let mut supported = 0;
        #[allow(unsafe_code)]
        // SAFETY: `supported` is valid for the call.
        let status = unsafe { library.NvEncodeAPIGetMaxSupportedVersion(&raw mut supported) };
        check(status, "version query")?;
        // Reported as major << 4 | minor.
        if supported < (sys::NVENCAPI_MAJOR_VERSION << 4 | sys::NVENCAPI_MINOR_VERSION) {
            return Err(NvencError(
                "the NVIDIA driver is too old (520 or newer needed)".into(),
            ));
        }
        let mut api = sys::NV_ENCODE_API_FUNCTION_LIST {
            version: FUNCTION_LIST_VER,
            ..sys::NV_ENCODE_API_FUNCTION_LIST::default()
        };
        #[allow(unsafe_code)]
        // SAFETY: `api` is a versioned function list the call fills in.
        let status = unsafe { library.NvEncodeAPICreateInstance(&raw mut api) };
        check(status, "create instance")?;
        let mut encoder = Self {
            api,
            encoder: std::ptr::null_mut(),
            input: std::ptr::null_mut(),
            output: std::ptr::null_mut(),
            settings,
            frames: 0,
            _library: library,
            cuda,
        };
        // From here on, Drop releases whatever was created.
        encoder.open_session()?;
        Ok(encoder)
    }

    fn missing() -> NvencError {
        NvencError("the driver's function list is incomplete".into())
    }

    fn open_session(&mut self) -> Result<(), NvencError> {
        let open = self
            .api
            .nvEncOpenEncodeSessionEx
            .ok_or_else(Self::missing)?;
        let mut params = sys::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS {
            version: OPEN_SESSION_VER,
            deviceType: consts::DEVICE_CUDA,
            device: self.cuda.context.cast::<c_void>(),
            apiVersion: API_VERSION,
            ..sys::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS::default()
        };
        #[allow(unsafe_code)]
        // SAFETY: `params` names a live CUDA context; `encoder` is written.
        let status = unsafe { open(&raw mut params, &raw mut self.encoder) };
        check(status, "open session")
    }

    /// The low-latency preset, adjusted: Main profile, CABAC, constant bit
    /// rate with a one-frame buffer, a keyframe every `keyframe_interval`
    /// frames, headers before every keyframe, and BT.601 colours.
    fn config(&self) -> Result<sys::NV_ENC_CONFIG, NvencError> {
        let get_preset = self
            .api
            .nvEncGetEncodePresetConfigEx
            .ok_or_else(Self::missing)?;
        let mut preset = sys::NV_ENC_PRESET_CONFIG {
            version: PRESET_CONFIG_VER,
            presetCfg: sys::NV_ENC_CONFIG {
                version: CONFIG_VER,
                ..sys::NV_ENC_CONFIG::default()
            },
            ..sys::NV_ENC_PRESET_CONFIG::default()
        };
        #[allow(unsafe_code)]
        // SAFETY: the session is open; `preset` is versioned and written.
        let status = unsafe {
            get_preset(
                self.encoder,
                CODEC_H264,
                PRESET_P4,
                sys::NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                &raw mut preset,
            )
        };
        check(status, "preset")?;
        let s = self.settings;
        let mut config = preset.presetCfg;
        config.version = CONFIG_VER;
        config.profileGUID = PROFILE_MAIN;
        config.gopLength = s.keyframe_interval;
        config.frameIntervalP = 1;
        config.rcParams.version = RC_PARAMS_VER;
        config.rcParams.rateControlMode = consts::CBR;
        config.rcParams.averageBitRate = s.bits_per_second;
        config.rcParams.maxBitRate = s.bits_per_second;
        config.rcParams.vbvBufferSize = s.bits_per_second / s.frame_rate.max(1);
        config.rcParams.vbvInitialDelay = config.rcParams.vbvBufferSize;
        #[allow(unsafe_code)]
        // SAFETY: the preset was asked for the H.264 codec, so the union's
        // H.264 member is the one the driver filled in.
        let mut h264 = unsafe { config.encodeCodecConfig.h264Config };
        h264.idrPeriod = s.keyframe_interval;
        h264.set_repeatSPSPPS(1);
        h264.entropyCodingMode = consts::CABAC;
        let vui = &mut h264.h264VUIParameters;
        vui.videoSignalTypePresentFlag = 1;
        vui.videoFormat = 5; // unspecified
        vui.videoFullRangeFlag = 0;
        vui.colourDescriptionPresentFlag = 1;
        vui.colourPrimaries = 6; // BT.601 (SMPTE 170M)
        vui.transferCharacteristics = 6;
        vui.colourMatrix = 6;
        config.encodeCodecConfig = sys::NV_ENC_CODEC_CONFIG { h264Config: h264 };
        Ok(config)
    }

    fn initialise(&mut self) -> Result<(), NvencError> {
        let mut config = self.config()?;
        let initialise = self.api.nvEncInitializeEncoder.ok_or_else(Self::missing)?;
        let s = self.settings;
        let mut params = sys::NV_ENC_INITIALIZE_PARAMS {
            version: INITIALIZE_PARAMS_VER,
            encodeGUID: CODEC_H264,
            presetGUID: PRESET_P4,
            encodeWidth: s.width,
            encodeHeight: s.height,
            darWidth: s.width,
            darHeight: s.height,
            frameRateNum: s.frame_rate,
            frameRateDen: 1,
            enablePTD: 1,
            encodeConfig: &raw mut config,
            maxEncodeWidth: s.width,
            maxEncodeHeight: s.height,
            tuningInfo: sys::NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
            ..sys::NV_ENC_INITIALIZE_PARAMS::default()
        };
        #[allow(unsafe_code)]
        // SAFETY: `params` and the config it points to live for the call.
        let status = unsafe { initialise(self.encoder, &raw mut params) };
        check(status, "initialise")
    }

    fn create_buffers(&mut self) -> Result<(), NvencError> {
        let create_input = self.api.nvEncCreateInputBuffer.ok_or_else(Self::missing)?;
        let create_output = self
            .api
            .nvEncCreateBitstreamBuffer
            .ok_or_else(Self::missing)?;
        let mut input = sys::NV_ENC_CREATE_INPUT_BUFFER {
            version: CREATE_INPUT_BUFFER_VER,
            width: self.settings.width,
            height: self.settings.height,
            bufferFmt: consts::NV12,
            ..sys::NV_ENC_CREATE_INPUT_BUFFER::default()
        };
        #[allow(unsafe_code)]
        // SAFETY: the encoder is initialised; `input` is written.
        let status = unsafe { create_input(self.encoder, &raw mut input) };
        check(status, "input buffer")?;
        self.input = input.inputBuffer;
        let mut output = sys::NV_ENC_CREATE_BITSTREAM_BUFFER {
            version: CREATE_BITSTREAM_BUFFER_VER,
            ..sys::NV_ENC_CREATE_BITSTREAM_BUFFER::default()
        };
        #[allow(unsafe_code)]
        // SAFETY: as above; `output` is written.
        let status = unsafe { create_output(self.encoder, &raw mut output) };
        check(status, "output buffer")?;
        self.output = output.bitstreamBuffer;
        Ok(())
    }

    /// Encodes one NV12 picture (`width × height`, rows packed). The first
    /// frame, and every `keyframe_interval`-th, is a keyframe with headers.
    ///
    /// # Errors
    /// [`NvencError`] when the picture has the wrong size or the GPU fails.
    pub fn encode(&mut self, nv12: &[u8]) -> Result<Vec<u8>, NvencError> {
        let (width, height) = (self.settings.width as usize, self.settings.height as usize);
        if nv12.len() != width * height * 3 / 2 {
            return Err(NvencError("picture size doesn't match the stream".into()));
        }
        let pitch = self.upload(nv12, width, height)?;
        let encode = self.api.nvEncEncodePicture.ok_or_else(Self::missing)?;
        let keyframe = self
            .frames
            .is_multiple_of(self.settings.keyframe_interval.max(1));
        let mut picture = sys::NV_ENC_PIC_PARAMS {
            version: PIC_PARAMS_VER,
            inputWidth: self.settings.width,
            inputHeight: self.settings.height,
            inputPitch: pitch,
            encodePicFlags: if keyframe { FORCE_KEYFRAME } else { 0 },
            frameIdx: self.frames,
            inputTimeStamp: u64::from(self.frames),
            inputBuffer: self.input,
            outputBitstream: self.output,
            bufferFmt: consts::NV12,
            pictureStruct: consts::FRAME,
            ..sys::NV_ENC_PIC_PARAMS::default()
        };
        self.frames = self.frames.wrapping_add(1);
        #[allow(unsafe_code)]
        // SAFETY: the input buffer holds the picture (unlocked) and the
        // output buffer is free; NVENC encodes synchronously.
        let status = unsafe { encode(self.encoder, &raw mut picture) };
        check(status, "encode")?;
        self.read_output()
    }

    /// Copies the picture into NVENC's input buffer; returns its pitch.
    fn upload(&mut self, nv12: &[u8], width: usize, height: usize) -> Result<u32, NvencError> {
        let lock = self.api.nvEncLockInputBuffer.ok_or_else(Self::missing)?;
        let unlock = self.api.nvEncUnlockInputBuffer.ok_or_else(Self::missing)?;
        let mut params = sys::NV_ENC_LOCK_INPUT_BUFFER {
            version: LOCK_INPUT_BUFFER_VER,
            inputBuffer: self.input,
            ..sys::NV_ENC_LOCK_INPUT_BUFFER::default()
        };
        #[allow(unsafe_code)]
        // SAFETY: the buffer belongs to this session and isn't locked.
        let status = unsafe { lock(self.encoder, &raw mut params) };
        check(status, "lock input")?;
        let pitch = params.pitch as usize;
        let result = if params.bufferDataPtr.is_null() || pitch < width {
            Err(NvencError("input buffer has no room".into()))
        } else {
            // NV12 in NVENC's buffers: luma rows, then chroma at pitch × height.
            #[allow(unsafe_code)]
            // SAFETY: a locked NV12 buffer holds `pitch × height × 3 / 2`
            // bytes, writable until the unlock below.
            let destination = unsafe {
                std::slice::from_raw_parts_mut(
                    params.bufferDataPtr.cast::<u8>(),
                    pitch * height * 3 / 2,
                )
            };
            copy_nv12(nv12, width, height, pitch, destination);
            Ok(params.pitch)
        };
        #[allow(unsafe_code)]
        // SAFETY: locked above; `destination` is out of scope.
        let status = unsafe { unlock(self.encoder, self.input) };
        let pitch = result?;
        check(status, "unlock input")?;
        Ok(pitch)
    }

    fn read_output(&mut self) -> Result<Vec<u8>, NvencError> {
        let lock = self.api.nvEncLockBitstream.ok_or_else(Self::missing)?;
        let unlock = self.api.nvEncUnlockBitstream.ok_or_else(Self::missing)?;
        let mut params = sys::NV_ENC_LOCK_BITSTREAM {
            version: LOCK_BITSTREAM_VER,
            outputBitstream: self.output,
            ..sys::NV_ENC_LOCK_BITSTREAM::default()
        };
        #[allow(unsafe_code)]
        // SAFETY: the output buffer belongs to this session; locking waits
        // for the frame.
        let status = unsafe { lock(self.encoder, &raw mut params) };
        check(status, "lock output")?;
        let stream = if params.bitstreamBufferPtr.is_null() {
            Vec::new()
        } else {
            #[allow(unsafe_code)]
            // SAFETY: a locked bitstream holds `bitstreamSizeInBytes` bytes
            // at `bitstreamBufferPtr` until the unlock below.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    params.bitstreamBufferPtr.cast::<u8>(),
                    params.bitstreamSizeInBytes as usize,
                )
            };
            bytes.to_vec()
        };
        #[allow(unsafe_code)]
        // SAFETY: locked above; `bytes` is out of scope.
        let status = unsafe { unlock(self.encoder, self.output) };
        check(status, "unlock output")?;
        Ok(stream)
    }
}

/// Copies packed NV12 rows into a buffer whose rows are `pitch` bytes
/// apart, chroma starting at `pitch × height`. Pure, so it's tested without
/// a GPU; `destination` must hold `pitch × height × 3 / 2` bytes.
fn copy_nv12(nv12: &[u8], width: usize, height: usize, pitch: usize, destination: &mut [u8]) {
    for (row, line) in nv12.chunks_exact(width).take(height * 3 / 2).enumerate() {
        if let Some(target) = destination.get_mut(row * pitch..row * pitch + width) {
            target.copy_from_slice(line);
        }
    }
}

impl Drop for NvencEncoder {
    fn drop(&mut self) {
        if self.encoder.is_null() {
            return;
        }
        if let (false, Some(destroy)) = (self.input.is_null(), self.api.nvEncDestroyInputBuffer) {
            #[allow(unsafe_code)]
            // SAFETY: created in `create_buffers`, destroyed once.
            unsafe {
                destroy(self.encoder, self.input);
            }
        }
        if let (false, Some(destroy)) =
            (self.output.is_null(), self.api.nvEncDestroyBitstreamBuffer)
        {
            #[allow(unsafe_code)]
            // SAFETY: created in `create_buffers`, destroyed once.
            unsafe {
                destroy(self.encoder, self.output);
            }
        }
        if let Some(destroy) = self.api.nvEncDestroyEncoder {
            #[allow(unsafe_code)]
            // SAFETY: the session was opened in `open_session`; its buffers
            // are gone; destroyed once, before the CUDA context.
            unsafe {
                destroy(self.encoder);
            }
        }
    }
}

/// Whether this computer has an NVIDIA GPU that NVENC can use: opens a
/// session and closes it straight away, without setting up an encoder or
/// buffers (consumer GPUs allow only a few sessions at once).
#[must_use]
pub fn available() -> bool {
    NvencEncoder::open(Settings {
        width: 256,
        height: 256,
        bits_per_second: 1_000_000,
        frame_rate: 30,
        keyframe_interval: 30,
    })
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_versions_match_the_headers_formula() {
        // NVENCAPI_STRUCT_VERSION(1) for API 12.0: 12 | 1 << 16 | 7 << 28.
        assert_eq!(OPEN_SESSION_VER, 0x7001_000c);
        assert_eq!(CONFIG_VER, 0xf008_000c);
    }

    #[test]
    fn nv12_rows_land_at_the_pitch() {
        // 2×2: two luma rows, one chroma row.
        let nv12 = [1, 2, 3, 4, 5, 6];
        let mut destination = [0u8; 12];
        copy_nv12(&nv12, 2, 2, 4, &mut destination);
        assert_eq!(destination, [1, 2, 0, 0, 3, 4, 0, 0, 5, 6, 0, 0]);
    }

    #[test]
    fn without_an_nvidia_driver_it_says_so() {
        if std::path::Path::new("/proc/driver/nvidia").exists() {
            return;
        }
        assert!(
            NvencEncoder::new(Settings {
                width: 64,
                height: 64,
                bits_per_second: 1_000_000,
                frame_rate: 30,
                keyframe_interval: 30,
            })
            .is_err()
        );
    }
}
