//! Checking the screen layouts viewers ask for (`SetDesktopSize`).

use farsight_rfb::Screen;

use crate::MAX_SIDE;

/// Most screens (monitors) one desktop may have.
pub const MAX_SCREENS: usize = 16;

/// Why a layout was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    /// The desktop is empty or larger than [`MAX_SIDE`].
    #[error("invalid desktop size")]
    Size,
    /// More than [`MAX_SCREENS`] screens.
    #[error("too many screens")]
    TooManyScreens,
    /// A screen is empty or reaches outside the desktop.
    #[error("a screen is outside the desktop")]
    OutOfBounds,
    /// Two screens overlap or share an id.
    #[error("screens overlap")]
    Overlap,
}

/// Most pixels a desktop may have (8192 × 4096, about 33.5 million): each
/// frame is 4 bytes a pixel, held in several places at once.
pub const MAX_PIXELS: u32 = 8192 * 4096;

/// Whether `width` × `height` is a desktop size the compositor accepts.
pub(crate) fn valid_size(width: u16, height: u16) -> bool {
    (1..=MAX_SIDE).contains(&width)
        && (1..=MAX_SIDE).contains(&height)
        && u32::from(width) * u32::from(height) <= MAX_PIXELS
}

/// Checks a requested layout and returns the screens to create: the ones
/// given, or a single screen covering the desktop when none are.
///
/// # Errors
/// [`LayoutError`] describing the first problem found.
pub fn validate_layout(
    width: u16,
    height: u16,
    screens: &[Screen],
) -> Result<Vec<Screen>, LayoutError> {
    if !valid_size(width, height) {
        return Err(LayoutError::Size);
    }
    if screens.is_empty() {
        return Ok(vec![Screen {
            id: 0,
            x: 0,
            y: 0,
            width,
            height,
            flags: 0,
        }]);
    }
    if screens.len() > MAX_SCREENS {
        return Err(LayoutError::TooManyScreens);
    }
    for screen in screens {
        let right = u32::from(screen.x) + u32::from(screen.width);
        let bottom = u32::from(screen.y) + u32::from(screen.height);
        if screen.width == 0
            || screen.height == 0
            || right > u32::from(width)
            || bottom > u32::from(height)
        {
            return Err(LayoutError::OutOfBounds);
        }
    }
    for (i, a) in screens.iter().enumerate() {
        for b in screens.iter().skip(i + 1) {
            if a.id == b.id || overlaps(a, b) {
                return Err(LayoutError::Overlap);
            }
        }
    }
    Ok(screens.to_vec())
}

fn overlaps(a: &Screen, b: &Screen) -> bool {
    let (ax2, ay2) = (
        u32::from(a.x) + u32::from(a.width),
        u32::from(a.y) + u32::from(a.height),
    );
    let (bx2, by2) = (
        u32::from(b.x) + u32::from(b.width),
        u32::from(b.y) + u32::from(b.height),
    );
    u32::from(a.x) < bx2 && u32::from(b.x) < ax2 && u32::from(a.y) < by2 && u32::from(b.y) < ay2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(id: u32, x: u16, y: u16, width: u16, height: u16) -> Screen {
        Screen {
            id,
            x,
            y,
            width,
            height,
            flags: 0,
        }
    }

    #[test]
    fn no_screens_means_one_covering_the_desktop() {
        assert_eq!(
            validate_layout(800, 600, &[]),
            Ok(vec![screen(0, 0, 0, 800, 600)])
        );
    }

    #[test]
    fn side_by_side_monitors_are_accepted() {
        let layout = [screen(1, 0, 0, 1920, 1080), screen(2, 1920, 0, 1280, 1024)];
        assert_eq!(validate_layout(3200, 1080, &layout), Ok(layout.to_vec()));
    }

    #[test]
    fn bad_layouts_are_refused() {
        assert_eq!(validate_layout(0, 600, &[]), Err(LayoutError::Size));
        assert_eq!(
            validate_layout(MAX_SIDE + 1, 600, &[]),
            Err(LayoutError::Size)
        );
        assert_eq!(
            validate_layout(MAX_SIDE, MAX_SIDE, &[]),
            Err(LayoutError::Size),
            "too many pixels"
        );
        assert!(validate_layout(MAX_SIDE, 4096, &[]).is_ok());
        assert_eq!(
            validate_layout(800, 600, &[screen(1, 700, 0, 200, 600)]),
            Err(LayoutError::OutOfBounds)
        );
        assert_eq!(
            validate_layout(800, 600, &[screen(1, 0, 0, 0, 600)]),
            Err(LayoutError::OutOfBounds)
        );
        assert_eq!(
            validate_layout(
                800,
                600,
                &[screen(1, 0, 0, 500, 600), screen(2, 400, 0, 400, 600)]
            ),
            Err(LayoutError::Overlap)
        );
        assert_eq!(
            validate_layout(
                800,
                600,
                &[screen(1, 0, 0, 400, 600), screen(1, 400, 0, 400, 600)]
            ),
            Err(LayoutError::Overlap)
        );
        let many: Vec<_> = (0..17).map(|i| screen(i, 0, 0, 1, 1)).collect();
        assert_eq!(
            validate_layout(800, 600, &many),
            Err(LayoutError::TooManyScreens)
        );
    }
}
