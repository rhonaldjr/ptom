//! Small view helpers shared by the home screen and tabs.

use cosmic::iced;
use cosmic::iced::{Alignment, Length, Point, Size, mouse};
use cosmic::prelude::*;
use cosmic::widget::{self, image};
use farsight_client::Fingerprint;
use farsight_net::CursorImage;

/// Centres content in the available space.
pub fn centred<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    widget::container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .padding(24)
        .into()
}

/// SHA-256 as colon-separated hex, in two lines of 16 bytes.
pub fn format_fingerprint(fingerprint: &Fingerprint) -> String {
    fingerprint
        .chunks(16)
        .map(|half| {
            half.iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clamps a float into the `u16` range.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Clamped to the u16 range first.
pub fn to_u16(value: f32) -> u16 {
    value.clamp(0.0, f32::from(u16::MAX)) as u16
}

/// Draws the remote cursor shape at the local pointer position.
pub struct CursorOverlay {
    pub cursor: Option<(image::Handle, CursorImage)>,
    pub position: Option<Point>,
    pub scale: f32,
}

impl<M> iced::widget::canvas::Program<M, cosmic::Theme> for CursorOverlay {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &cosmic::Renderer,
        _theme: &cosmic::Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<iced::widget::canvas::Geometry> {
        let mut frame = iced::widget::canvas::Frame::new(renderer, bounds.size());
        if let (Some((handle, cursor)), Some(position)) = (&self.cursor, self.position) {
            #[allow(clippy::cast_precision_loss)] // Cursors are at most 256 pixels.
            let (w, h, hx, hy) = (
                cursor.width as f32 * self.scale,
                cursor.height as f32 * self.scale,
                f32::from(cursor.hotspot.0) * self.scale,
                f32::from(cursor.hotspot.1) * self.scale,
            );
            let area = iced::Rectangle::new(
                Point::new(position.x - hx, position.y - hy),
                Size::new(w, h),
            );
            frame.draw_image(area, iced::widget::canvas::Image::new(handle.clone()));
        }
        vec![frame.into_geometry()]
    }
}

/// Maps widget coordinates to framebuffer coordinates for a letterboxed image.
#[derive(Debug, Clone, Copy)]
pub struct Mapping {
    pub scale: f32,
    offset_x: f32,
    offset_y: f32,
    width: u32,
    height: u32,
}

impl Mapping {
    pub fn new(size: Size, width: u32, height: u32) -> Self {
        #[allow(clippy::cast_precision_loss)] // Screen sizes are far below 2^24.
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let scale = (size.width / w).min(size.height / h);
        Self {
            scale,
            offset_x: (size.width - w * scale) / 2.0,
            offset_y: (size.height - h * scale) / 2.0,
            width,
            height,
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Clamped to 0..=u16::MAX first.
    pub fn to_remote(self, point: Point) -> (u16, u16) {
        let clamp = |v: f32, max: u32| -> u16 {
            #[allow(clippy::cast_precision_loss)]
            let max = max.saturating_sub(1).min(u32::from(u16::MAX)) as f32;
            v.clamp(0.0, max) as u16
        };
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        (
            clamp((point.x - self.offset_x) / scale, self.width),
            clamp((point.y - self.offset_y) / scale, self.height),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterboxed_mapping_clamps_to_the_framebuffer() {
        let m = Mapping::new(Size::new(200.0, 100.0), 100, 100);
        assert_eq!(m.to_remote(Point::new(100.0, 50.0)), (50, 50));
        assert_eq!(m.to_remote(Point::new(0.0, 0.0)), (0, 0));
        assert_eq!(m.to_remote(Point::new(1000.0, 1000.0)), (99, 99));
    }

    #[test]
    fn fingerprint_formatting() {
        let text = format_fingerprint(&[0xab; 32]);
        assert_eq!(text.lines().count(), 2);
        assert!(text.starts_with("AB:AB"));
    }
}
