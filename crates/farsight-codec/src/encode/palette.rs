//! Colour analysis shared by the ZRLE and Tight encoders.

use std::collections::HashMap;

/// The distinct raw pixel values of an area, in order of first appearance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Palette {
    colours: Vec<u32>,
    index: HashMap<u32, u8>,
}

impl Palette {
    /// Collects the distinct values of `raw`, or `None` if there are more
    /// than `max` (at most 255) of them.
    pub(crate) fn collect(raw: &[u32], max: usize) -> Option<Self> {
        let max = max.min(255);
        let mut palette = Self {
            colours: Vec::new(),
            index: HashMap::new(),
        };
        let mut last = None;
        for &pixel in raw {
            if last == Some(pixel) {
                continue;
            }
            last = Some(pixel);
            if palette.index.contains_key(&pixel) {
                continue;
            }
            if palette.colours.len() == max {
                return None;
            }
            let index = u8::try_from(palette.colours.len()).ok()?;
            palette.index.insert(pixel, index);
            palette.colours.push(pixel);
        }
        Some(palette)
    }

    /// Number of colours.
    pub(crate) fn len(&self) -> usize {
        self.colours.len()
    }

    /// The colours, in index order.
    pub(crate) fn colours(&self) -> &[u32] {
        &self.colours
    }

    /// The colour at `index` (0 if out of range).
    pub(crate) fn colour(&self, index: usize) -> u32 {
        self.colours.get(index).copied().unwrap_or(0)
    }

    /// The index of `pixel`, which must be in the palette (0 otherwise).
    pub(crate) fn index(&self, pixel: u32) -> u8 {
        self.index.get(&pixel).copied().unwrap_or(0)
    }
}

/// Runs of equal values: `(value, length)`, lengths at least 1.
pub(crate) fn runs(raw: &[u32]) -> impl Iterator<Item = (u32, usize)> + '_ {
    raw.chunk_by(|a, b| a == b)
        .filter_map(|run| run.first().map(|&value| (value, run.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_in_order_and_gives_up_past_the_limit() {
        let p = Palette::collect(&[5, 5, 3, 5, 9], 3).unwrap();
        assert_eq!(p.colours(), [5, 3, 9]);
        assert_eq!((p.index(3), p.index(9), p.colour(0)), (1, 2, 5));
        assert!(Palette::collect(&[1, 2, 3, 4], 3).is_none());
    }

    #[test]
    fn runs_split_on_changes() {
        let all: Vec<_> = runs(&[1, 1, 2, 1, 1, 1]).collect();
        assert_eq!(all, [(1, 2), (2, 1), (1, 3)]);
        assert_eq!(runs(&[]).count(), 0);
    }
}
