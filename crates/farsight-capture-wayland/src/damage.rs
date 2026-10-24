//! Damage regions: converting `SPA_META_VideoDamage` into RFB rectangles and carrying the
//! damage of dropped frames forward, so a throttled frame never hides a change.

use farsight_rfb::Rect;

/// Beyond this many rectangles we report the whole frame as damaged instead.
pub const MAX_RECTS: usize = 64;

/// One damage region as the compositor reports it (it may lie partly off-frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// Clamps `region` to a `width` × `height` frame; `None` if nothing is left.
#[must_use]
pub fn clamp(region: Region, width: u16, height: u16) -> Option<Rect> {
    let (x0, x1) = clamp_span(region.x, region.width, width)?;
    let (y0, y1) = clamp_span(region.y, region.height, height)?;
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

fn clamp_span(start: i32, len: u32, limit: u16) -> Option<(u16, u16)> {
    let start = i64::from(start);
    let end = start + i64::from(len);
    let limit_i = i64::from(limit);
    let lo = u16::try_from(start.clamp(0, limit_i)).ok()?;
    let hi = u16::try_from(end.clamp(0, limit_i)).ok()?;
    (hi > lo).then_some((lo, hi))
}

/// Converts the compositor's regions into rectangles inside a `width` × `height` frame.
///
/// Returns `None` ("the whole frame") when there are no usable regions or too many of them,
/// because an empty or unbounded list from the compositor is not something to trust.
#[must_use]
pub fn convert(
    regions: impl IntoIterator<Item = Region>,
    width: u16,
    height: u16,
) -> Option<Vec<Rect>> {
    let mut rects = Vec::new();
    for region in regions {
        if let Some(rect) = clamp(region, width, height) {
            if rects.len() == MAX_RECTS {
                return None;
            }
            rects.push(rect);
        }
    }
    (!rects.is_empty()).then_some(rects)
}

/// Collects damage across frames that were not delivered.
///
/// `None` damage means the whole frame. The accumulator starts as "whole frame" so the first
/// delivered frame is always complete.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Accumulator {
    /// Nothing changed since the last delivered frame.
    Clean,
    /// These rectangles changed.
    Rects(Vec<Rect>),
    /// Everything may have changed.
    #[default]
    Whole,
}

impl Accumulator {
    /// Marks the whole frame dirty (e.g. after a resize).
    pub fn reset(&mut self) {
        *self = Self::Whole;
    }

    /// Adds the damage of a frame that is being skipped or delivered.
    pub fn add(&mut self, damage: Option<Vec<Rect>>) {
        *self = match (std::mem::take(self), damage) {
            (Self::Whole, _) | (_, None) => Self::Whole,
            (Self::Clean, Some(rects)) => Self::Rects(rects),
            (Self::Rects(mut pending), Some(more)) => {
                if pending.len() + more.len() > MAX_RECTS {
                    Self::Whole
                } else {
                    pending.extend(more);
                    Self::Rects(pending)
                }
            }
        };
    }

    /// The damage to deliver with this frame (`None` = whole frame), leaving it clean.
    pub fn take(&mut self) -> Option<Vec<Rect>> {
        match std::mem::replace(self, Self::Clean) {
            Self::Clean => Some(Vec::new()),
            Self::Rects(rects) => Some(rects),
            Self::Whole => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn region(x: i32, y: i32, width: u32, height: u32) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn clamps_to_the_frame() {
        assert_eq!(
            clamp(region(10, 10, 5, 5), 100, 100),
            Some(Rect::new(10, 10, 5, 5))
        );
        assert_eq!(
            clamp(region(-5, -5, 10, 10), 100, 100),
            Some(Rect::new(0, 0, 5, 5))
        );
        assert_eq!(
            clamp(region(95, 95, 10, 10), 100, 100),
            Some(Rect::new(95, 95, 5, 5))
        );
        assert_eq!(clamp(region(100, 0, 10, 10), 100, 100), None);
        assert_eq!(clamp(region(0, 0, 0, 10), 100, 100), None);
        assert_eq!(
            clamp(region(i32::MIN, i32::MAX, u32::MAX, u32::MAX), 100, 100),
            None
        );
        assert_eq!(
            clamp(region(i32::MIN, 0, u32::MAX, 1), 100, 100),
            Some(Rect::new(0, 0, 100, 1))
        );
    }

    #[test]
    fn empty_or_excessive_damage_means_whole_frame() {
        assert_eq!(convert([], 10, 10), None);
        assert_eq!(convert([region(50, 50, 1, 1)], 10, 10), None);
        let many = (0..=MAX_RECTS).map(|_| region(0, 0, 1, 1));
        assert_eq!(convert(many, 10, 10), None);
        assert_eq!(
            convert([region(1, 2, 3, 4), region(50, 50, 1, 1)], 10, 10),
            Some(vec![Rect::new(1, 2, 3, 4)])
        );
    }

    #[test]
    fn accumulator_starts_full_and_merges() {
        let mut acc = Accumulator::default();
        acc.add(Some(vec![Rect::new(0, 0, 1, 1)]));
        assert_eq!(acc.take(), None, "first frame is always whole");

        acc.add(Some(vec![Rect::new(0, 0, 1, 1)]));
        acc.add(Some(vec![Rect::new(2, 2, 1, 1)]));
        assert_eq!(
            acc.take(),
            Some(vec![Rect::new(0, 0, 1, 1), Rect::new(2, 2, 1, 1)])
        );

        acc.add(Some(vec![Rect::new(0, 0, 1, 1)]));
        acc.add(None);
        acc.add(Some(vec![Rect::new(2, 2, 1, 1)]));
        assert_eq!(acc.take(), None, "whole-frame damage absorbs the rest");

        acc.add(Some(vec![Rect::new(3, 3, 1, 1)]));
        acc.reset();
        assert_eq!(acc.take(), None, "reset marks the whole frame");
    }

    #[test]
    fn accumulator_overflows_to_whole_frame() {
        let mut acc = Accumulator::default();
        acc.take();
        for _ in 0..=MAX_RECTS {
            acc.add(Some(vec![Rect::new(0, 0, 1, 1)]));
        }
        assert_eq!(acc.take(), None);
    }

    proptest! {
        #[test]
        fn converted_rects_fit_the_frame(
            regions in proptest::collection::vec(
                (any::<i32>(), any::<i32>(), any::<u32>(), any::<u32>()), 0..80),
            width in 0u16..4096,
            height in 0u16..4096,
        ) {
            let regions = regions.into_iter().map(|(x, y, w, h)| region(x, y, w, h));
            if let Some(rects) = convert(regions, width, height) {
                prop_assert!(!rects.is_empty() && rects.len() <= MAX_RECTS);
                for rect in rects {
                    prop_assert!(!rect.is_empty());
                    prop_assert!(rect.fits_within(width, height));
                }
            }
        }
    }
}
