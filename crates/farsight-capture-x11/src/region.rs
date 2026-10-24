//! Damage bookkeeping: clipping, coalescing and tile comparison.

use farsight_rfb::Rect;

/// Above this many rectangles a frame reports one bounding box instead.
pub(crate) const MAX_RECTS: usize = 32;

/// Edge length of the comparison tiles used when polling.
pub(crate) const TILE: u16 = 32;

/// Clips a signed X rectangle to a `width` × `height` screen.
pub(crate) fn clip(x: i16, y: i16, w: u16, h: u16, width: u16, height: u16) -> Option<Rect> {
    let left = i32::from(x).max(0);
    let top = i32::from(y).max(0);
    let right = (i32::from(x) + i32::from(w)).min(i32::from(width));
    let bottom = (i32::from(y) + i32::from(h)).min(i32::from(height));
    if right <= left || bottom <= top {
        return None;
    }
    Some(Rect::new(
        u16::try_from(left).ok()?,
        u16::try_from(top).ok()?,
        u16::try_from(right - left).ok()?,
        u16::try_from(bottom - top).ok()?,
    ))
}

/// The smallest rectangle containing both `a` and `b`.
pub(crate) fn union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    let right = a.right().max(b.right());
    let bottom = a.bottom().max(b.bottom());
    Rect::new(
        x,
        y,
        u16::try_from(right - u32::from(x)).unwrap_or(u16::MAX),
        u16::try_from(bottom - u32::from(y)).unwrap_or(u16::MAX),
    )
}

fn bounding_box(rects: &[Rect]) -> Option<Rect> {
    rects.iter().copied().reduce(union)
}

/// Merges damage into a short list of rectangles.
///
/// Pairs whose bounding box costs no more pixels than the two rectangles
/// separately (overlapping, contained or touching) are merged. If more than
/// [`MAX_RECTS`] remain, the result is their single bounding box.
pub(crate) fn coalesce(rects: &[Rect]) -> Vec<Rect> {
    let mut out: Vec<Rect> = rects.iter().copied().filter(|r| !r.is_empty()).collect();
    if out.len() > MAX_RECTS * 8 {
        return bounding_box(&out).into_iter().collect();
    }
    while let Some((i, j)) = find_mergeable(&out) {
        let b = out.swap_remove(j);
        if let Some(a) = out.get_mut(i) {
            *a = union(*a, b);
        }
    }
    if out.len() > MAX_RECTS {
        return bounding_box(&out).into_iter().collect();
    }
    out
}

fn find_mergeable(rects: &[Rect]) -> Option<(usize, usize)> {
    for (i, a) in rects.iter().enumerate() {
        for (j, b) in rects.iter().enumerate().skip(i + 1) {
            if union(*a, *b).area() <= a.area() + b.area() {
                return Some((i, j));
            }
        }
    }
    None
}

/// Compares two `width` × `height` frames in [`TILE`]-sized tiles and returns
/// the changed tiles, with horizontally adjacent ones joined into runs.
/// Frames of the wrong size count as entirely changed.
pub(crate) fn changed_tiles(old: &[u32], new: &[u32], width: u16, height: u16) -> Vec<Rect> {
    let len = usize::from(width) * usize::from(height);
    if old.len() != len || new.len() != len {
        return vec![Rect::new(0, 0, width, height)];
    }
    let mut out = Vec::new();
    for ty in (0..height).step_by(usize::from(TILE)) {
        let th = TILE.min(height - ty);
        let mut run: Option<Rect> = None;
        for tx in (0..width).step_by(usize::from(TILE)) {
            let tile = Rect::new(tx, ty, TILE.min(width - tx), th);
            if tile_differs(old, new, width, tile) {
                run = Some(run.map_or(tile, |r| union(r, tile)));
            } else if let Some(r) = run.take() {
                out.push(r);
            }
        }
        out.extend(run);
    }
    coalesce(&out)
}

fn tile_differs(old: &[u32], new: &[u32], width: u16, tile: Rect) -> bool {
    (0..tile.height).any(|row| {
        let start = usize::from(tile.y + row) * usize::from(width) + usize::from(tile.x);
        let range = start..start + usize::from(tile.width);
        old.get(range.clone()) != new.get(range)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_trims_to_screen() {
        assert_eq!(clip(-5, -5, 10, 10, 100, 100), Some(Rect::new(0, 0, 5, 5)));
        assert_eq!(
            clip(95, 90, 10, 20, 100, 100),
            Some(Rect::new(95, 90, 5, 10))
        );
        assert_eq!(clip(100, 0, 10, 10, 100, 100), None);
        assert_eq!(clip(-20, 0, 10, 10, 100, 100), None);
    }

    #[test]
    fn coalesce_merges_overlapping_and_adjacent() {
        let merged = coalesce(&[
            Rect::new(0, 0, 10, 10),
            Rect::new(10, 0, 10, 10),
            Rect::new(2, 2, 3, 3),
        ]);
        assert_eq!(merged, [Rect::new(0, 0, 20, 10)]);
    }

    #[test]
    fn coalesce_keeps_distant_rects_apart() {
        let rects = [Rect::new(0, 0, 4, 4), Rect::new(50, 50, 4, 4)];
        assert_eq!(coalesce(&rects), rects);
    }

    #[test]
    fn coalesce_drops_empty_and_caps_count() {
        assert_eq!(coalesce(&[Rect::new(3, 3, 0, 9)]), Vec::new());
        let many: Vec<Rect> = (0..40).map(|i| Rect::new(i * 10, i * 10, 2, 2)).collect();
        assert_eq!(coalesce(&many), [Rect::new(0, 0, 392, 392)]);
    }

    #[test]
    fn tile_diff_reports_changed_tiles_only() {
        let (w, h) = (100, 70);
        let old = vec![0; 100 * 70];
        let mut new = old.clone();
        new[5 * 100 + 40] = 1; // tile (32, 0)
        new[69 * 100 + 99] = 2; // bottom-right partial tile (96, 64)
        assert_eq!(
            changed_tiles(&old, &new, w, h),
            [Rect::new(32, 0, 32, 32), Rect::new(96, 64, 4, 6)]
        );
        assert_eq!(changed_tiles(&old, &old, w, h), Vec::new());
    }

    #[test]
    fn tile_diff_joins_adjacent_tiles() {
        let old = vec![0; 96 * 32];
        let mut new = old.clone();
        new[0] = 1;
        new[40] = 1;
        assert_eq!(changed_tiles(&old, &new, 96, 32), [Rect::new(0, 0, 64, 32)]);
    }

    #[test]
    fn tile_diff_with_size_mismatch_is_full() {
        assert_eq!(
            changed_tiles(&[0; 4], &[0; 6], 3, 2),
            [Rect::new(0, 0, 3, 2)]
        );
    }
}
