//! The Farsight logos as tray pixmaps, built into the binary so the tray
//! shows them whether or not the icon theme has them (e.g. the `.tar.gz`
//! download). Rendered from `dist/icons/*.svg` by `scripts/render-icons.sh`.

/// Which logo a tray shows.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Logo {
    Viewer,
    Sharing,
    /// Sharing, with a red dot: someone is watching.
    SharingActive,
}

/// The logo at 32 and 64 pixels, as `StatusNotifierItem` wants it.
pub(crate) fn pixmaps(logo: Logo) -> Vec<ksni::Icon> {
    let (small, large): (&[u8], &[u8]) = match logo {
        Logo::Viewer => (
            include_bytes!("../../../dist/icons/tray/viewer-32.rgba"),
            include_bytes!("../../../dist/icons/tray/viewer-64.rgba"),
        ),
        Logo::Sharing => (
            include_bytes!("../../../dist/icons/tray/sharing-32.rgba"),
            include_bytes!("../../../dist/icons/tray/sharing-64.rgba"),
        ),
        Logo::SharingActive => (
            include_bytes!("../../../dist/icons/tray/sharing-active-32.rgba"),
            include_bytes!("../../../dist/icons/tray/sharing-active-64.rgba"),
        ),
    };
    vec![pixmap(32, small), pixmap(64, large)]
}

/// RGBA bytes to the ARGB (network byte order) that the tray protocol uses.
fn pixmap(size: i32, rgba: &[u8]) -> ksni::Icon {
    let (pixels, _) = rgba.as_chunks::<4>();
    let data = pixels
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect();
    ksni::Icon {
        width: size,
        height: size,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_logo_has_square_pixmaps_of_the_right_size() {
        for logo in [Logo::Viewer, Logo::Sharing, Logo::SharingActive] {
            for icon in pixmaps(logo) {
                let pixels = usize::try_from(icon.width * icon.height).unwrap_or(0);
                assert_eq!(icon.data.len(), pixels * 4);
            }
        }
    }

    #[test]
    fn pixels_are_reordered_to_argb() {
        let icon = pixmap(1, &[1, 2, 3, 4]);
        assert_eq!(icon.data, [4, 1, 2, 3]);
    }
}
