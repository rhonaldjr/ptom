//! Damage tracking: which parts of the framebuffer changed.
//!
//! [`diff`] finds changes between two frames on a 64×64 tile grid, for
//! capture backends that cannot report damage themselves. [`Region`] is a
//! bounded set of rectangles: past its limit it merges the pair of
//! rectangles whose union wastes the least area, so the work per update
//! stays bounded however fragmented the damage is.

use farsight_codec::Framebuffer;
use farsight_rfb::Rect;

/// Side of the square tiles [`diff`] compares.
pub const TILE: u16 = 64;

/// The intersection of `a` and `b`, if they overlap.
#[must_use]
pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    if u32::from(left) >= right || u32::from(top) >= bottom {
        return None;
    }
    Some(Rect::new(
        left,
        top,
        u16::try_from(right - u32::from(left)).ok()?,
        u16::try_from(bottom - u32::from(top)).ok()?,
    ))
}

/// The smallest rectangle containing `a` and `b` (clamped to the
/// coordinate space).
#[must_use]
pub fn bounding(a: Rect, b: Rect) -> Rect {
    let left = a.x.min(b.x);
    let top = a.y.min(b.y);
    let right = a.right().max(b.right());
    let bottom = a.bottom().max(b.bottom());
    let side = |from: u16, to: u32| u16::try_from(to - u32::from(from)).unwrap_or(u16::MAX);
    Rect::new(left, top, side(left, right), side(top, bottom))
}

fn contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

/// `rect` cut into pieces at most `max_width` × `max_height`, row by row.
pub fn split(rect: Rect, max_width: u16, max_height: u16) -> impl Iterator<Item = Rect> {
    let (w_step, h_step) = (max_width.max(1), max_height.max(1));
    (0..rect.height)
        .step_by(usize::from(h_step))
        .flat_map(move |dy| {
            (0..rect.width).step_by(usize::from(w_step)).map(move |dx| {
                Rect::new(
                    rect.x.saturating_add(dx),
                    rect.y.saturating_add(dy),
                    w_step.min(rect.width - dx),
                    h_step.min(rect.height - dy),
                )
            })
        })
}

/// The parts of `rect` outside `hole` (up to four pieces).
fn subtract(rect: Rect, hole: Rect) -> Vec<Rect> {
    let Some(cut) = intersect(rect, hole) else {
        return vec![rect];
    };
    let mut out = Vec::with_capacity(4);
    let piece = |x: u16, y: u16, right: u32, bottom: u32| -> Option<Rect> {
        let w = u16::try_from(right.checked_sub(u32::from(x))?).ok()?;
        let h = u16::try_from(bottom.checked_sub(u32::from(y))?).ok()?;
        let r = Rect::new(x, y, w, h);
        (!r.is_empty()).then_some(r)
    };
    // Above and below the cut span the full width; left and right only its rows.
    out.extend(piece(rect.x, rect.y, rect.right(), u32::from(cut.y)));
    if let Ok(below) = u16::try_from(cut.bottom()) {
        out.extend(piece(rect.x, below, rect.right(), rect.bottom()));
    }
    out.extend(piece(rect.x, cut.y, u32::from(cut.x), cut.bottom()));
    if let Ok(after) = u16::try_from(cut.right()) {
        out.extend(piece(after, cut.y, rect.right(), cut.bottom()));
    }
    out
}

/// Finds the 64×64 tiles that differ between `old` and `new`, merged into
/// horizontal runs. Frames of different sizes differ everywhere.
#[must_use]
pub fn diff(old: &Framebuffer, new: &Framebuffer) -> Vec<Rect> {
    let full = Rect::new(0, 0, new.width(), new.height());
    if (old.width(), old.height()) != (new.width(), new.height()) {
        return if full.is_empty() {
            Vec::new()
        } else {
            vec![full]
        };
    }
    let stride = usize::from(new.width());
    tile_diff(full, |x, y, w| {
        let start = usize::from(y) * stride + usize::from(x);
        let end = start + usize::from(w);
        old.pixels().get(start..end) != new.pixels().get(start..end)
    })
}

/// Like [`diff`] but within `area` only, where `before` holds the previous
/// pixels of `area` (row-major, `area.width` per row).
pub(crate) fn diff_area(before: &[u32], new: &Framebuffer, area: Rect) -> Vec<Rect> {
    let stride = usize::from(new.width());
    let area_stride = usize::from(area.width);
    tile_diff(area, |x, y, w| {
        let start = usize::from(y) * stride + usize::from(x);
        let old_start = usize::from(y - area.y) * area_stride + usize::from(x - area.x);
        before.get(old_start..old_start + usize::from(w))
            != new.pixels().get(start..start + usize::from(w))
    })
}

/// Walks the grid-aligned tiles covering `area`; `row_differs(x, y, w)`
/// says whether one row segment of a tile changed.
fn tile_diff(area: Rect, mut row_differs: impl FnMut(u16, u16, u16) -> bool) -> Vec<Rect> {
    let mut out = Vec::new();
    if area.is_empty() {
        return out;
    }
    let first_col = area.x / TILE;
    let first_row = area.y / TILE;
    let last_col = u16::try_from((area.right() - 1) / u32::from(TILE)).unwrap_or(u16::MAX);
    let last_row = u16::try_from((area.bottom() - 1) / u32::from(TILE)).unwrap_or(u16::MAX);
    for row in first_row..=last_row {
        let mut run: Option<Rect> = None;
        for col in first_col..=last_col {
            let grid = Rect::new(col * TILE, row * TILE, TILE, TILE);
            let Some(tile) = intersect(grid, area) else {
                continue;
            };
            let dirty = (tile.y..)
                .take(usize::from(tile.height))
                .any(|y| row_differs(tile.x, y, tile.width));
            match (dirty, run) {
                (true, Some(r)) => run = Some(bounding(r, tile)),
                (true, None) => run = Some(tile),
                (false, Some(r)) => {
                    out.push(r);
                    run = None;
                }
                (false, None) => {}
            }
        }
        out.extend(run);
    }
    out
}

/// Default limit on rectangles in a [`Region`].
pub const DEFAULT_MAX_RECTS: usize = 32;

/// A set of damaged rectangles, coalesced to at most a fixed number.
///
/// Rectangles may overlap; consumers only need every damaged pixel covered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    rects: Vec<Rect>,
    max_rects: usize,
}

impl Default for Region {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_RECTS)
    }
}

impl Region {
    /// An empty region holding at most `max_rects` (at least 1) rectangles.
    #[must_use]
    pub fn new(max_rects: usize) -> Self {
        Self {
            rects: Vec::new(),
            max_rects: max_rects.max(1),
        }
    }

    /// The rectangles.
    #[must_use]
    pub fn rects(&self) -> &[Rect] {
        &self.rects
    }

    /// Whether nothing is damaged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// Forgets all damage.
    pub fn clear(&mut self) {
        self.rects.clear();
    }

    /// Takes the rectangles, leaving the region empty.
    pub fn take(&mut self) -> Vec<Rect> {
        std::mem::take(&mut self.rects)
    }

    /// Adds `rect` (empty rectangles are ignored).
    pub fn add(&mut self, rect: Rect) {
        if rect.is_empty() || self.rects.iter().any(|&r| contains(r, rect)) {
            return;
        }
        self.rects.retain(|&r| !contains(rect, r));
        self.rects.push(rect);
        self.coalesce();
    }

    /// Adds every rectangle in `rects`.
    pub fn add_all(&mut self, rects: impl IntoIterator<Item = Rect>) {
        for rect in rects {
            self.add(rect);
        }
    }

    /// Removes `hole` from the region (the damage there was just sent).
    ///
    /// The remaining pieces are not merged back over the hole unless the
    /// region grows past four times its limit, so sent areas are not
    /// re-sent on the next incremental update.
    pub fn subtract(&mut self, hole: Rect) {
        if hole.is_empty() {
            return;
        }
        let old = std::mem::take(&mut self.rects);
        for rect in old {
            for piece in subtract(rect, hole) {
                if !self.rects.iter().any(|&r| contains(r, piece)) {
                    self.rects.push(piece);
                }
            }
        }
        if self.rects.len() > self.max_rects.saturating_mul(4) {
            self.coalesce();
        }
    }

    /// Clips every rectangle to `bounds`, dropping those outside it.
    pub fn clip(&mut self, bounds: Rect) {
        let old = std::mem::take(&mut self.rects);
        self.add_all(old.into_iter().filter_map(|r| intersect(r, bounds)));
    }

    /// The parts of the region inside `area`.
    #[must_use]
    pub fn within(&self, area: Rect) -> Vec<Rect> {
        self.rects
            .iter()
            .filter_map(|&r| intersect(r, area))
            .collect()
    }

    /// Merges the cheapest pairs until the limit holds.
    fn coalesce(&mut self) {
        while self.rects.len() > self.max_rects {
            let mut best: Option<(usize, usize, usize)> = None;
            for (i, &a) in self.rects.iter().enumerate() {
                for (j, &b) in self.rects.iter().enumerate().skip(i + 1) {
                    let waste = bounding(a, b).area().saturating_sub(a.area() + b.area());
                    if best.is_none_or(|(_, _, w)| waste < w) {
                        best = Some((i, j, waste));
                    }
                }
            }
            let Some((i, j, _)) = best else { return };
            let b = self.rects.swap_remove(j);
            let a = self.rects.swap_remove(i);
            self.add(bounding(a, b));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn diff_reports_changed_tiles_as_runs() {
        let old = Framebuffer::new(200, 130);
        let mut new = old.clone();
        new.fill(Rect::new(10, 10, 1, 1), 1).unwrap();
        new.fill(Rect::new(70, 10, 1, 1), 1).unwrap();
        new.fill(Rect::new(199, 129, 1, 1), 1).unwrap();
        assert_eq!(
            diff(&old, &new),
            [Rect::new(0, 0, 128, 64), Rect::new(192, 128, 8, 2)]
        );
        assert_eq!(diff(&old, &old), []);
        assert_eq!(
            diff(&Framebuffer::new(1, 1), &new),
            [Rect::new(0, 0, 200, 130)]
        );
    }

    #[test]
    fn diff_area_compares_against_the_previous_pixels() {
        let mut fb = Framebuffer::new(100, 100);
        let area = Rect::new(60, 60, 10, 10);
        let before = fb.read(area).unwrap();
        fb.fill(Rect::new(65, 65, 1, 1), 9).unwrap();
        assert_eq!(diff_area(&before, &fb, area), [Rect::new(64, 64, 6, 6)]);
        assert_eq!(diff_area(&fb.read(area).unwrap(), &fb, area), []);
    }

    #[test]
    fn region_coalesces_and_subtracts() {
        let mut r = Region::new(2);
        r.add(Rect::new(0, 0, 10, 10));
        r.add(Rect::new(2, 2, 3, 3));
        assert_eq!(r.rects(), [Rect::new(0, 0, 10, 10)]);
        r.add(Rect::new(100, 0, 10, 10));
        r.add(Rect::new(12, 0, 10, 10));
        assert_eq!(r.rects().len(), 2);
        assert!(r.rects().contains(&Rect::new(0, 0, 22, 10)));
        r.subtract(Rect::new(0, 0, 200, 5));
        assert!(r.rects().iter().all(|x| x.y >= 5));
        r.clip(Rect::new(0, 0, 50, 50));
        assert_eq!(r.rects(), [Rect::new(0, 5, 22, 5)]);
    }

    #[test]
    fn split_covers_with_bounded_pieces() {
        let pieces: Vec<_> = split(Rect::new(5, 5, 300, 10), 256, 256).collect();
        assert_eq!(
            pieces,
            [Rect::new(5, 5, 256, 10), Rect::new(261, 5, 44, 10)]
        );
    }

    fn rect() -> impl Strategy<Value = Rect> {
        (0u16..60, 0u16..60, 0u16..30, 0u16..30).prop_map(|(x, y, w, h)| Rect::new(x, y, w, h))
    }

    fn covered(rects: &[Rect], x: u16, y: u16) -> bool {
        rects.iter().any(|r| contains(*r, Rect::new(x, y, 1, 1)))
    }

    proptest! {
        #[test]
        fn region_never_loses_damage(adds in proptest::collection::vec(rect(), 0..20), hole in rect(), max in 1usize..6) {
            let mut region = Region::new(max);
            region.add_all(adds.iter().copied());
            prop_assert!(region.rects().len() <= max);
            for &r in &adds {
                for (x, y) in [(r.x, r.y), (r.x + r.width.saturating_sub(1), r.y + r.height.saturating_sub(1))] {
                    if !r.is_empty() {
                        prop_assert!(covered(region.rects(), x, y));
                    }
                }
            }
            let before = region.clone();
            region.subtract(hole);
            for y in 0..90 {
                for x in 0..90 {
                    let in_hole = contains(hole, Rect::new(x, y, 1, 1));
                    let was = covered(before.rects(), x, y);
                    if in_hole {
                        prop_assert!(!covered(region.rects(), x, y) || hole.is_empty());
                    } else if was {
                        prop_assert!(covered(region.rects(), x, y));
                    }
                }
            }
        }
    }
}
