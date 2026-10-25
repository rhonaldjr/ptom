//! The H.264 headers a hardware encoder stream needs, written here (from
//! ITU-T H.264 §7.3) so every GPU gets the same, known-good syntax: the
//! sequence and picture parameter sets and each slice header. The stream is
//! Main profile, CABAC, progressive, one reference frame, no B-frames, and
//! `pic_order_cnt_type` 2 (display order = decode order), so viewers can
//! show each frame as soon as it arrives.

/// Writes bits most-significant first, with Exp-Golomb codes.
#[derive(Debug, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    /// Bits used in the last byte (0 = the last byte is full / none yet).
    used: u8,
}

impl BitWriter {
    /// Appends the low `count` bits of `value` (`count` ≤ 32).
    pub fn bits(&mut self, value: u32, count: u8) {
        for shift in (0..count).rev() {
            let bit = u8::from((value >> shift) & 1 == 1);
            if self.used == 0 {
                self.bytes.push(0);
            }
            if let Some(last) = self.bytes.last_mut() {
                *last |= bit << (7 - self.used);
            }
            self.used = (self.used + 1) % 8;
        }
    }

    /// One flag bit.
    pub fn flag(&mut self, on: bool) {
        self.bits(u32::from(on), 1);
    }

    /// Unsigned Exp-Golomb (`ue(v)`).
    pub fn ue(&mut self, value: u32) {
        let coded = u64::from(value) + 1;
        let length = u8::try_from(64 - coded.leading_zeros()).unwrap_or(33);
        // `length - 1` zeros, then `coded` in `length` bits.
        for _ in 1..length {
            self.bits(0, 1);
        }
        for shift in (0..length).rev() {
            self.bits(u32::from((coded >> shift) & 1 == 1), 1);
        }
    }

    /// Signed Exp-Golomb (`se(v)`): 0, 1, −1, 2, −2…
    pub fn se(&mut self, value: i32) {
        let mapped = if value > 0 {
            value.unsigned_abs() * 2 - 1
        } else {
            value.unsigned_abs() * 2
        };
        self.ue(mapped);
    }

    /// Bits written so far.
    #[must_use]
    pub fn len_bits(&self) -> usize {
        let partial = if self.used == 0 {
            0
        } else {
            8 - usize::from(self.used)
        };
        self.bytes.len() * 8 - partial
    }

    /// `rbsp_trailing_bits`: a one, then zeros to the byte boundary.
    pub fn trailing(&mut self) {
        self.bits(1, 1);
        while self.used != 0 {
            self.bits(0, 1);
        }
    }

    /// The bytes written (the last may be partly used).
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A NAL unit: start code, header byte, and the payload with emulation
/// prevention (`00 00 0x` → `00 00 03 0x`, x ≤ 3).
#[must_use]
pub fn nal(ref_idc: u8, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 1, ((ref_idc & 3) << 5) | (kind & 0x1f)];
    let mut zeros = 0;
    for &byte in payload {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// NAL unit types used here.
const NAL_SLICE: u8 = 1;
const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;
/// Main profile.
const PROFILE_MAIN: u32 = 77;

/// The stream's fixed parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream {
    /// Visible width in pixels (even).
    pub width: u16,
    /// Visible height in pixels (even).
    pub height: u16,
    /// `level_idc`.
    pub level_idc: u8,
    /// `log2_max_frame_num_minus4`.
    pub log2_max_frame_num_minus4: u8,
    /// The PPS's starting QP.
    pub initial_qp: u8,
}

impl Stream {
    /// For `width × height` (even sizes); `None` if too large for H.264
    /// level 5.1 (4096×2304).
    #[must_use]
    pub fn new(width: u16, height: u16) -> Option<Self> {
        let (width_mbs, height_mbs) = (
            u32::from(width.div_ceil(16)),
            u32::from(height.div_ceil(16)),
        );
        // Annex A: MaxFS bounds the area, and each side to √(8 × MaxFS).
        let level_idc = [(8192, 256, 41), (22080, 420, 50), (36864, 543, 51)]
            .into_iter()
            .find(|&(frame_size, side, _)| {
                width_mbs * height_mbs <= frame_size && width_mbs <= side && height_mbs <= side
            })
            .map(|(_, _, level)| level)?;
        if width_mbs == 0 || height_mbs == 0 {
            return None;
        }
        Some(Self {
            width,
            height,
            level_idc,
            // frame_num counts to 512, beyond the keyframe interval.
            log2_max_frame_num_minus4: 5,
            initial_qp: 26,
        })
    }

    /// Width in macroblocks.
    #[must_use]
    pub fn width_mbs(&self) -> u16 {
        self.width.div_ceil(16)
    }

    /// Height in macroblocks.
    #[must_use]
    pub fn height_mbs(&self) -> u16 {
        self.height.div_ceil(16)
    }

    /// Coded pixels beyond the visible right edge.
    #[must_use]
    pub fn crop_right(&self) -> u16 {
        self.width_mbs() * 16 - self.width
    }

    /// Coded pixels beyond the visible bottom edge.
    #[must_use]
    pub fn crop_bottom(&self) -> u16 {
        self.height_mbs() * 16 - self.height
    }

    /// The sequence parameter set NAL unit, ready to send.
    #[must_use]
    pub fn sequence_parameter_set(&self) -> Vec<u8> {
        nal(3, NAL_SPS, &self.sps())
    }

    /// The picture parameter set NAL unit, ready to send.
    #[must_use]
    pub fn picture_parameter_set(&self) -> Vec<u8> {
        nal(3, NAL_PPS, &self.pps())
    }

    fn sps(self) -> Vec<u8> {
        let mut w = BitWriter::default();
        w.bits(PROFILE_MAIN, 8);
        // constraint_set1_flag: the stream obeys Main profile.
        w.bits(0b0100_0000, 8);
        w.bits(u32::from(self.level_idc), 8);
        w.ue(0); // seq_parameter_set_id
        w.ue(u32::from(self.log2_max_frame_num_minus4));
        w.ue(2); // pic_order_cnt_type
        w.ue(1); // max_num_ref_frames
        w.flag(false); // gaps_in_frame_num_value_allowed_flag
        w.ue(u32::from(self.width_mbs()) - 1);
        w.ue(u32::from(self.height_mbs()) - 1);
        w.flag(true); // frame_mbs_only_flag
        w.flag(true); // direct_8x8_inference_flag
        let cropped = self.crop_right() > 0 || self.crop_bottom() > 0;
        w.flag(cropped);
        if cropped {
            // In 2-pixel units for 4:2:0.
            w.ue(0);
            w.ue(u32::from(self.crop_right() / 2));
            w.ue(0);
            w.ue(u32::from(self.crop_bottom() / 2));
        }
        w.flag(true); // vui_parameters_present_flag
        Self::vui(&mut w);
        w.trailing();
        w.into_bytes()
    }

    /// Colours (limited-range BT.601, as converted) and "no reordering",
    /// so decoders output every frame immediately.
    fn vui(w: &mut BitWriter) {
        w.flag(false); // aspect_ratio_info_present_flag
        w.flag(false); // overscan_info_present_flag
        w.flag(true); // video_signal_type_present_flag
        w.bits(5, 3); // video_format: unspecified
        w.flag(false); // video_full_range_flag
        w.flag(true); // colour_description_present_flag
        w.bits(6, 8); // colour_primaries: BT.601 (SMPTE 170M)
        w.bits(6, 8); // transfer_characteristics
        w.bits(6, 8); // matrix_coefficients
        w.flag(false); // chroma_loc_info_present_flag
        w.flag(false); // timing_info_present_flag
        w.flag(false); // nal_hrd_parameters_present_flag
        w.flag(false); // vcl_hrd_parameters_present_flag
        w.flag(false); // pic_struct_present_flag
        w.flag(true); // bitstream_restriction_flag
        w.flag(true); // motion_vectors_over_pic_boundaries_flag
        w.ue(0); // max_bytes_per_pic_denom
        w.ue(0); // max_bits_per_mb_denom
        w.ue(16); // log2_max_mv_length_horizontal
        w.ue(16); // log2_max_mv_length_vertical
        w.ue(0); // max_num_reorder_frames
        w.ue(1); // max_dec_frame_buffering
    }

    fn pps(self) -> Vec<u8> {
        let mut w = BitWriter::default();
        w.ue(0); // pic_parameter_set_id
        w.ue(0); // seq_parameter_set_id
        w.flag(true); // entropy_coding_mode_flag: CABAC
        w.flag(false); // bottom_field_pic_order_in_frame_present_flag
        w.ue(0); // num_slice_groups_minus1
        w.ue(0); // num_ref_idx_l0_default_active_minus1
        w.ue(0); // num_ref_idx_l1_default_active_minus1
        w.flag(false); // weighted_pred_flag
        w.bits(0, 2); // weighted_bipred_idc
        w.se(i32::from(self.initial_qp) - 26); // pic_init_qp_minus26
        w.se(0); // pic_init_qs_minus26
        w.se(0); // chroma_qp_index_offset
        w.flag(true); // deblocking_filter_control_present_flag
        w.flag(false); // constrained_intra_pred_flag
        w.flag(false); // redundant_pic_cnt_present_flag
        w.trailing();
        w.into_bytes()
    }

    /// The slice header for `frame` (one slice per picture) and its length
    /// in bits; the encoder appends the slice data right after.
    #[must_use]
    pub fn slice_header(&self, frame: SliceFrame) -> (Vec<u8>, u32) {
        let mut w = BitWriter::default();
        w.ue(0); // first_mb_in_slice
        w.ue(if frame.idr { 7 } else { 5 }); // slice_type: I / P, all slices
        w.ue(0); // pic_parameter_set_id
        w.bits(
            u32::from(frame.frame_num),
            self.log2_max_frame_num_minus4 + 4,
        );
        if frame.idr {
            w.ue(u32::from(frame.idr_pic_id));
        } else {
            w.flag(false); // num_ref_idx_active_override_flag
            w.flag(false); // ref_pic_list_modification_flag_l0
        }
        // dec_ref_pic_marking (every frame is a reference).
        if frame.idr {
            w.flag(false); // no_output_of_prior_pics_flag
            w.flag(false); // long_term_reference_flag
        } else {
            w.flag(false); // adaptive_ref_pic_marking_mode_flag
            w.ue(0); // cabac_init_idc (CABAC, P slices)
        }
        w.se(0); // slice_qp_delta
        w.ue(0); // disable_deblocking_filter_idc
        w.se(0); // slice_alpha_c0_offset_div2
        w.se(0); // slice_beta_offset_div2
        let payload_bits = w.len_bits();
        let kind = if frame.idr { NAL_IDR } else { NAL_SLICE };
        let bytes = nal(3, kind, &w.into_bytes());
        // Start code and NAL header (5 bytes), the payload's bits, and any
        // emulation-prevention bytes inserted into it.
        let payload_bytes = payload_bits.div_ceil(8);
        let inserted = bytes.len() - 5 - payload_bytes;
        let bits = (5 + inserted) * 8 + payload_bits;
        (bytes, u32::try_from(bits).unwrap_or(u32::MAX))
    }
}

/// A frame's position, for its slice header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceFrame {
    /// A keyframe.
    pub idr: bool,
    /// `frame_num`.
    pub frame_num: u16,
    /// `idr_pic_id`.
    pub idr_pic_id: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads bits back, for checking what was written.
    struct BitReader<'a> {
        bytes: &'a [u8],
        position: usize,
    }

    impl BitReader<'_> {
        fn bit(&mut self) -> u32 {
            let byte = self.bytes[self.position / 8];
            self.position += 1;
            u32::from((byte >> (7 - (self.position - 1) % 8)) & 1)
        }
        fn bits(&mut self, count: u8) -> u32 {
            (0..count).fold(0, |acc, _| (acc << 1) | self.bit())
        }
        fn ue(&mut self) -> u32 {
            let mut zeros = 0;
            while self.bit() == 0 {
                zeros += 1;
            }
            (1 << zeros) - 1 + self.bits(zeros)
        }
    }

    #[test]
    fn exp_golomb_codes_match_the_standard() {
        let mut w = BitWriter::default();
        for value in [0, 1, 2, 3, 7] {
            w.ue(value);
        }
        // 1 010 011 00100 0001000
        assert_eq!(w.len_bits(), 1 + 3 + 3 + 5 + 7);
        assert_eq!(w.into_bytes(), [0b1010_0110, 0b0100_0001, 0b0000_0000]);
        let mut w = BitWriter::default();
        w.se(1);
        w.se(-1);
        // 010 011
        assert_eq!(w.into_bytes(), [0b0100_1100]);
    }

    #[test]
    fn emulation_prevention_breaks_up_start_code_patterns() {
        assert_eq!(
            nal(3, 7, &[0, 0, 1, 0, 0, 0]),
            [0, 0, 0, 1, 0x67, 0, 0, 3, 1, 0, 0, 3, 0]
        );
        assert_eq!(nal(0, 1, &[0, 0, 4]), [0, 0, 0, 1, 1, 0, 0, 4]);
    }

    #[test]
    fn levels_follow_the_picture_size() {
        assert_eq!(Stream::new(1920, 1080).map(|s| s.level_idc), Some(41));
        assert_eq!(Stream::new(2560, 1440).map(|s| s.level_idc), Some(50));
        assert_eq!(Stream::new(3840, 2160).map(|s| s.level_idc), Some(51));
        assert_eq!(Stream::new(7680, 4320), None);
        assert_eq!(Stream::new(0, 0), None);
        // A very wide desktop needs a higher level for its width alone.
        assert_eq!(Stream::new(5120, 720).map(|s| s.level_idc), Some(50));
        assert_eq!(Stream::new(16384, 64), None);
    }

    #[test]
    fn the_sps_carries_the_size_and_cropping() {
        let stream = Stream::new(1920, 1080).unwrap();
        let sets = stream.sequence_parameter_set();
        assert_eq!(&sets[..5], [0, 0, 0, 1, 0x67]);
        let mut r = BitReader {
            bytes: &sets[5..],
            position: 0,
        };
        assert_eq!(r.bits(8), 77);
        r.bits(16);
        assert_eq!(r.ue(), 0);
        assert_eq!(r.ue(), 5);
        assert_eq!(r.ue(), 2);
        assert_eq!(r.ue(), 1);
        r.bit();
        assert_eq!(r.ue() + 1, 120);
        assert_eq!(r.ue() + 1, 68);
        r.bits(2);
        assert_eq!(r.bit(), 1, "1080 isn't a multiple of 16: cropped");
        assert_eq!([r.ue(), r.ue(), r.ue(), r.ue()], [0, 0, 0, 4]);
    }

    #[test]
    fn slice_header_lengths_are_exact() {
        let stream = Stream::new(640, 360).unwrap();
        for frame in [
            SliceFrame {
                idr: true,
                frame_num: 0,
                idr_pic_id: 3,
            },
            SliceFrame {
                idr: false,
                frame_num: 17,
                idr_pic_id: 3,
            },
        ] {
            let (bytes, bits) = stream.slice_header(frame);
            assert_eq!(bytes[4], if frame.idr { 0x65 } else { 0x61 });
            assert!(bits as usize <= bytes.len() * 8 && bits as usize > (bytes.len() - 1) * 8);
            let mut r = BitReader {
                bytes: &bytes[5..],
                position: 0,
            };
            assert_eq!(r.ue(), 0);
            assert_eq!(r.ue(), if frame.idr { 7 } else { 5 });
            assert_eq!(r.ue(), 0);
            assert_eq!(r.bits(9), u32::from(frame.frame_num));
        }
    }
}
