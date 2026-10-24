//! Picking an encoding for each rectangle from what it contains (roadmap M8,
//! ENC-9): flat areas go to the cheapest fill, photo-like areas to JPEG when
//! the viewer asked for a quality level, everything else to the preferred
//! lossless encoding.

use std::collections::HashSet;

use farsight_rfb::Encoding;

/// Fewest pixels before JPEG is worth its header and blocking artefacts.
const JPEG_MIN_AREA: usize = 64 * 64;
/// Distinct colours (in a sample) above which an area counts as a photo.
const PHOTO_COLOURS: usize = 192;
/// Every n-th pixel is sampled when counting colours (keeps it cheap).
const SAMPLE_STEP: usize = 3;
/// Stop counting once this many distinct colours are found.
const COUNT_LIMIT: usize = 256;

/// What a rectangle looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Content {
    /// One colour.
    Solid,
    /// A handful of colours (text, UI).
    Flat,
    /// Many colours: a photo, video or gradient.
    Photo,
}

/// Classifies `pixels` (`0x00RRGGBB`) by sampling their colours.
pub(crate) fn analyse(pixels: &[u32]) -> Content {
    let Some(&first) = pixels.first() else {
        return Content::Solid;
    };
    if pixels.iter().all(|&p| p == first) {
        return Content::Solid;
    }
    let mut seen = HashSet::new();
    for &pixel in pixels.iter().step_by(SAMPLE_STEP) {
        seen.insert(pixel);
        if seen.len() >= COUNT_LIMIT {
            break;
        }
    }
    if pixels.len() >= JPEG_MIN_AREA && seen.len() >= PHOTO_COLOURS {
        Content::Photo
    } else {
        Content::Flat
    }
}

/// How a rectangle will be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Tight with JPEG at this quality level.
    TightJpeg(u8),
    /// This encoding, lossless.
    Lossless(Encoding),
}

/// What the viewer and the agent agreed on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Offer<'a> {
    /// Encodings both sides support, the agent's preference first.
    pub usable: &'a [Encoding],
    /// The viewer's JPEG quality level, if it sent one.
    pub jpeg_quality: Option<u8>,
    /// The viewer's pixel format can take JPEG.
    pub jpeg_capable: bool,
}

/// Picks an encoding for a rectangle with `content` covering `area` pixels.
pub(crate) fn choose(content: Content, area: usize, offer: Offer<'_>) -> Choice {
    let has = |e: Encoding| offer.usable.contains(&e);
    let preferred = offer.usable.first().copied().unwrap_or(Encoding::Raw);
    match content {
        // Tight sends a fill in a few bytes; RRE and Hextile nearly as few.
        Content::Solid => [
            Encoding::Tight,
            Encoding::Rre,
            Encoding::Hextile,
            Encoding::Zrle,
        ]
        .into_iter()
        .find(|&e| has(e))
        .map_or(Choice::Lossless(preferred), Choice::Lossless),
        Content::Photo if has(Encoding::Tight) && offer.jpeg_capable && area >= JPEG_MIN_AREA => {
            match offer.jpeg_quality {
                Some(level) => Choice::TightJpeg(level),
                None => Choice::Lossless(preferred),
            }
        }
        Content::Photo | Content::Flat => Choice::Lossless(preferred),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(n: usize) -> Vec<u32> {
        (0..n)
            .map(|i| u32::try_from(i * 2654 % 0xff_ffff).unwrap_or(0))
            .collect()
    }

    #[test]
    fn content_is_classified_by_colour_count() {
        assert_eq!(analyse(&[7; 100]), Content::Solid);
        assert_eq!(analyse(&[]), Content::Solid);
        let text: Vec<u32> = (0..10_000)
            .map(|i| if i % 7 == 0 { 0 } else { 0xff_ffff })
            .collect();
        assert_eq!(analyse(&text), Content::Flat);
        assert_eq!(analyse(&gradient(10_000)), Content::Photo);
        assert_eq!(
            analyse(&gradient(1_000)),
            Content::Flat,
            "too small for JPEG"
        );
    }

    #[test]
    fn choices_follow_content_and_what_the_viewer_allows() {
        let all = [
            Encoding::Tight,
            Encoding::Zrle,
            Encoding::Hextile,
            Encoding::Rre,
            Encoding::Raw,
        ];
        let offer = Offer {
            usable: &all,
            jpeg_quality: Some(7),
            jpeg_capable: true,
        };
        assert_eq!(choose(Content::Photo, 9_000, offer), Choice::TightJpeg(7));
        assert_eq!(
            choose(Content::Flat, 9_000, offer),
            Choice::Lossless(Encoding::Tight)
        );
        assert_eq!(
            choose(Content::Solid, 9_000, offer),
            Choice::Lossless(Encoding::Tight)
        );
        let no_quality = Offer {
            jpeg_quality: None,
            ..offer
        };
        assert_eq!(
            choose(Content::Photo, 9_000, no_quality),
            Choice::Lossless(Encoding::Tight)
        );
        let rgb565 = Offer {
            jpeg_capable: false,
            ..offer
        };
        assert_eq!(
            choose(Content::Photo, 9_000, rgb565),
            Choice::Lossless(Encoding::Tight)
        );
        let old_viewer = Offer {
            usable: &[Encoding::Hextile, Encoding::Rre, Encoding::Raw],
            ..offer
        };
        assert_eq!(
            choose(Content::Solid, 100, old_viewer),
            Choice::Lossless(Encoding::Rre)
        );
        assert_eq!(
            choose(Content::Photo, 9_000, old_viewer),
            Choice::Lossless(Encoding::Hextile)
        );
    }
}
