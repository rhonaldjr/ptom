//! `0x00RRGGBB` pixels to NV12 (a full-size Y plane, then interleaved U/V at
//! half size), limited-range BT.601: the colours the viewer's decoder
//! assumes. Integer arithmetic only, so it's fast enough per frame.

/// An NV12 picture with tightly packed rows (`width` bytes each).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nv12 {
    /// Width in pixels (even).
    pub width: usize,
    /// Height in pixels (even).
    pub height: usize,
    /// `width × height` luma bytes followed by `width × height / 2` chroma bytes.
    pub data: Vec<u8>,
}

impl Nv12 {
    /// The luma plane.
    #[must_use]
    pub fn y(&self) -> &[u8] {
        self.data
            .get(..self.width * self.height)
            .unwrap_or_default()
    }

    /// The interleaved chroma plane (U, V, U, V…).
    #[must_use]
    pub fn uv(&self) -> &[u8] {
        self.data
            .get(self.width * self.height..)
            .unwrap_or_default()
    }

    /// The picture enlarged to `width × height` (even, at least as large)
    /// by repeating its last column and row, so encoders that work in
    /// whole macroblocks don't spend bits on junk in the padding.
    #[must_use]
    pub fn padded(&self, width: usize, height: usize) -> Option<Self> {
        if width < self.width
            || height < self.height
            || !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
        {
            return None;
        }
        let mut data = Vec::with_capacity(width * height * 3 / 2);
        pad_plane(
            self.y(),
            self.width,
            self.height,
            1,
            width,
            height,
            &mut data,
        );
        pad_plane(
            self.uv(),
            self.width,
            self.height / 2,
            2,
            width,
            height / 2,
            &mut data,
        );
        Some(Self {
            width,
            height,
            data,
        })
    }
}

/// Appends `plane` (`width` bytes × `rows`) widened to `new_width` and
/// `new_rows`, repeating the last `unit` bytes of each row and the last row.
fn pad_plane(
    plane: &[u8],
    width: usize,
    rows: usize,
    unit: usize,
    new_width: usize,
    new_rows: usize,
    out: &mut Vec<u8>,
) {
    let start = out.len();
    for line in plane.chunks_exact(width).take(rows) {
        out.extend_from_slice(line);
        let edge = line.get(width.saturating_sub(unit)..).unwrap_or_default();
        out.extend(edge.iter().copied().cycle().take(new_width - width));
    }
    let last_row = out.len().saturating_sub(new_width).max(start);
    for _ in rows..new_rows {
        out.extend_from_within(last_row..last_row + new_width);
    }
}

fn channels(pixel: u32) -> [i32; 3] {
    let [_, r, g, b] = pixel.to_be_bytes();
    [i32::from(r), i32::from(g), i32::from(b)]
}

fn clamp_byte(value: i32) -> u8 {
    u8::try_from(value.clamp(0, 255)).unwrap_or(u8::MAX)
}

fn luma([r, g, b]: [i32; 3]) -> u8 {
    clamp_byte(16 + ((66 * r + 129 * g + 25 * b + 128) >> 8))
}

fn chroma([r, g, b]: [i32; 3]) -> [u8; 2] {
    [
        clamp_byte(128 + ((-38 * r - 74 * g + 112 * b + 128) >> 8)),
        clamp_byte(128 + ((112 * r - 94 * g - 18 * b + 128) >> 8)),
    ]
}

/// Converts a `width × height` picture (both even, `pixels.len()` equal to
/// their product); `None` otherwise.
#[must_use]
pub fn from_xrgb(pixels: &[u32], width: usize, height: usize) -> Option<Nv12> {
    if !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || pixels.len() != width.checked_mul(height)?
    {
        return None;
    }
    let mut data = Vec::with_capacity(width * height * 3 / 2);
    data.extend(pixels.iter().map(|&p| luma(channels(p))));
    for (top, bottom) in pixels
        .chunks_exact(width)
        .step_by(2)
        .zip(pixels.chunks_exact(width).skip(1).step_by(2))
    {
        for (above, below) in top.as_chunks::<2>().0.iter().zip(bottom.as_chunks::<2>().0) {
            let mut sum = [0i32; 3];
            for &pixel in above.iter().chain(below) {
                for (total, value) in sum.iter_mut().zip(channels(pixel)) {
                    *total += value;
                }
            }
            data.extend(chroma(sum.map(|total| (total + 2) / 4)));
        }
    }
    Some(Nv12 {
        width,
        height,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn black_white_and_red_match_bt601_limited_range() {
        let picture = from_xrgb(&[0x00_0000, 0xff_ffff, 0xff_0000, 0xff_0000], 2, 2).unwrap();
        assert_eq!(picture.y(), [16, 235, 82, 82]);
        // The rounded average of black, white and two reds.
        assert_eq!(picture.uv(), chroma([191, 64, 64]));
        let red = from_xrgb(&[0xff_0000; 4], 2, 2).unwrap();
        assert_eq!(red.uv(), [90, 240]);
    }

    #[test]
    fn padding_repeats_the_last_column_and_row() {
        // 2×2: luma 1 2 / 3 4, chroma (U, V) = (5, 6).
        let picture = Nv12 {
            width: 2,
            height: 2,
            data: vec![1, 2, 3, 4, 5, 6],
        };
        let padded = picture.padded(4, 4).unwrap();
        assert_eq!(padded.y(), [1, 2, 2, 2, 3, 4, 4, 4, 3, 4, 4, 4, 3, 4, 4, 4]);
        assert_eq!(padded.uv(), [5, 6, 5, 6, 5, 6, 5, 6]);
        assert!(picture.padded(1, 4).is_none());
    }

    #[test]
    fn odd_or_mismatched_sizes_are_refused() {
        assert!(from_xrgb(&[0; 3], 3, 1).is_none());
        assert!(from_xrgb(&[0; 4], 2, 4).is_none());
    }

    proptest! {
        #[test]
        fn output_is_always_one_and_a_half_bytes_per_pixel(
            half_w in 1usize..20, half_h in 1usize..20, seed in any::<u32>()
        ) {
            let (w, h) = (half_w * 2, half_h * 2);
            let pixels: Vec<u32> = (0..w * h).map(|i| seed.wrapping_mul(u32::try_from(i).unwrap() + 1)).collect();
            let picture = from_xrgb(&pixels, w, h).unwrap();
            prop_assert_eq!(picture.data.len(), w * h * 3 / 2);
            prop_assert!(picture.y().iter().all(|&y| (16..=235).contains(&y)));
        }
    }
}
