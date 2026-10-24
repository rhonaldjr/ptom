//! Covering an area with single-colour rectangles over a background, for
//! the RRE and Hextile encoders.

use std::collections::HashMap;

/// One single-colour rectangle, relative to the area's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Subrect {
    pub colour: u32,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// The most common value in `pixels` (ties: the first seen). `pixels` must
/// not be empty.
pub(crate) fn background(pixels: &[u32]) -> u32 {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    let mut best = (pixels.first().copied().unwrap_or(0), 0);
    for &pixel in pixels {
        let count = counts.entry(pixel).or_insert(0);
        *count += 1;
        if *count > best.1 {
            best = (pixel, *count);
        }
    }
    best.0
}

/// Greedily covers every pixel that isn't `background` with rectangles
/// (row runs grown downwards). Returns `None` once more than `max` are
/// needed, so callers can fall back to a cheaper encoding early.
pub(crate) fn find(
    pixels: &[u32],
    width: usize,
    background: u32,
    max: usize,
) -> Option<Vec<Subrect>> {
    let height = pixels.len().checked_div(width)?;
    let mut covered = vec![false; pixels.len()];
    let mut out = Vec::new();
    let at = |x: usize, y: usize| y * width + x;
    for y in 0..height {
        for x in 0..width {
            let i = at(x, y);
            let colour = pixels.get(i).copied()?;
            if colour == background || covered.get(i).copied().unwrap_or(true) {
                continue;
            }
            let same = |x: usize, y: usize| {
                pixels.get(at(x, y)) == Some(&colour) && covered.get(at(x, y)) == Some(&false)
            };
            let mut w = 1;
            while x + w < width && same(x + w, y) {
                w += 1;
            }
            let mut h = 1;
            while y + h < height && (x..x + w).all(|cx| same(cx, y + h)) {
                h += 1;
            }
            for row in y..y + h {
                for slot in covered.get_mut(at(x, row)..at(x, row) + w)? {
                    *slot = true;
                }
            }
            if out.len() == max {
                return None;
            }
            out.push(Subrect {
                colour,
                x,
                y,
                width: w,
                height: h,
            });
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subrects_cover_exactly_the_non_background_pixels() {
        #[rustfmt::skip]
        let pixels = [
            0, 1, 1, 0,
            0, 1, 1, 2,
            0, 0, 0, 2,
        ];
        let bg = background(&pixels);
        assert_eq!(bg, 0);
        let rects = find(&pixels, 4, bg, 10).unwrap();
        let mut painted = vec![bg; pixels.len()];
        for r in &rects {
            for y in r.y..r.y + r.height {
                for x in r.x..r.x + r.width {
                    painted[y * 4 + x] = r.colour;
                }
            }
        }
        assert_eq!(painted, pixels);
        assert_eq!(rects.len(), 2);
        assert!(find(&pixels, 4, bg, 1).is_none(), "gives up past max");
    }
}
