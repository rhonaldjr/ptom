//! The shared desktop model capture adapters feed and sessions read.

use std::collections::VecDeque;

use farsight_codec::{Cursor, Framebuffer};
use farsight_rfb::{DecodeError, Rect, Screen};

use crate::damage::{self, Region};

/// How many updates of damage history are kept. A session that falls
/// further behind than this simply repaints everything.
const HISTORY: usize = 64;
/// Rectangles kept per history entry.
const RECTS_PER_ENTRY: usize = 64;

/// What changed since a session last looked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Changes {
    /// These rectangles.
    Rects(Vec<Rect>),
    /// The session fell behind the history: assume everything.
    Everything,
}

#[derive(Debug, Clone)]
struct Entry {
    serial: u64,
    rects: Vec<Rect>,
}

/// The current state of the desktop being shared: framebuffer, cursor
/// shape and clipboard, plus a short history of damage.
///
/// Every [`crate::ServerSession`] keeps its own position in the history and
/// its own accumulated damage, so a slow viewer never holds back a fast one;
/// the adapter only calls [`crate::ServerSession::frame_ready`] for each
/// session after updating the desktop.
#[derive(Debug, Clone)]
pub struct Desktop {
    framebuffer: Framebuffer,
    name: String,
    cursor: Option<Cursor>,
    cursor_serial: u64,
    size_serial: u64,
    frame_serial: u64,
    history: VecDeque<Entry>,
    clipboard: Option<String>,
    clipboard_serial: u64,
}

impl Desktop {
    /// A black desktop of `width` × `height` called `name` (sent to viewers
    /// in `ServerInit`, after authentication and approval).
    #[must_use]
    pub fn new(width: u16, height: u16, name: impl Into<String>) -> Self {
        Self {
            framebuffer: Framebuffer::new(width, height),
            name: name.into(),
            cursor: None,
            cursor_serial: 0,
            size_serial: 0,
            frame_serial: 0,
            history: VecDeque::with_capacity(HISTORY),
            clipboard: None,
            clipboard_serial: 0,
        }
    }

    /// The current frame.
    #[must_use]
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.framebuffer
    }

    /// The desktop name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The screen layout: one screen covering the framebuffer.
    #[must_use]
    pub fn screens(&self) -> Vec<Screen> {
        vec![Screen {
            id: 0,
            x: 0,
            y: 0,
            width: self.framebuffer.width(),
            height: self.framebuffer.height(),
            flags: 0,
        }]
    }

    /// The cursor shape, if the adapter supplied one.
    #[must_use]
    pub fn cursor(&self) -> Option<&Cursor> {
        self.cursor.as_ref()
    }

    /// The last clipboard text set with [`Self::set_clipboard`].
    #[must_use]
    pub fn clipboard(&self) -> Option<&str> {
        self.clipboard.as_deref()
    }

    fn full(&self) -> Rect {
        Rect::new(0, 0, self.framebuffer.width(), self.framebuffer.height())
    }

    /// Replaces the whole frame. `damage` lists what changed; when the
    /// capture cannot tell, pass `None` and the core compares the frames on
    /// a 64×64 grid. A frame of a different size is a resize.
    pub fn update(&mut self, frame: Framebuffer, damage: Option<&[Rect]>) {
        let resized = (frame.width(), frame.height())
            != (self.framebuffer.width(), self.framebuffer.height());
        let rects = match damage {
            _ if resized => vec![Rect::new(0, 0, frame.width(), frame.height())],
            Some(rects) => rects.to_vec(),
            None => damage::diff(&self.framebuffer, &frame),
        };
        self.framebuffer = frame;
        if resized {
            self.size_serial += 1;
        }
        self.record(rects);
    }

    /// Writes `pixels` (row-major `0x00RRGGBB`, `rect.width` per row) into
    /// `rect`. `damage` as for [`Self::update`]; `None` compares the area
    /// against what was there.
    ///
    /// # Errors
    /// [`DecodeError::RectOutOfBounds`] if `rect` is outside the frame, or
    /// [`DecodeError::Malformed`] if `pixels` is not `rect`'s size.
    pub fn update_region(
        &mut self,
        rect: Rect,
        pixels: &[u32],
        damage: Option<&[Rect]>,
    ) -> Result<(), DecodeError> {
        if pixels.len() != rect.area() {
            return Err(DecodeError::Malformed(
                "pixel count does not match the area",
            ));
        }
        let before = self.framebuffer.read(rect)?;
        self.framebuffer
            .write(rect, |i| pixels.get(i).copied().unwrap_or(0))?;
        let rects = match damage {
            Some(rects) => rects.to_vec(),
            None => damage::diff_area(&before, &self.framebuffer, rect),
        };
        self.record(rects);
        Ok(())
    }

    /// Resizes to `width` × `height` (cleared to black) and damages it all.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.update(Framebuffer::new(width, height), None);
    }

    /// Sets the cursor shape; `None` hides the pointer on viewers that draw
    /// it locally.
    pub fn set_cursor(&mut self, cursor: Option<Cursor>) {
        self.cursor = cursor;
        self.cursor_serial += 1;
    }

    /// Sets the clipboard text that viewers receive.
    pub fn set_clipboard(&mut self, text: impl Into<String>) {
        self.clipboard = Some(text.into());
        self.clipboard_serial += 1;
    }

    fn record(&mut self, rects: Vec<Rect>) {
        let mut region = Region::new(RECTS_PER_ENTRY);
        let full = self.full();
        region.add_all(rects.into_iter().filter_map(|r| damage::intersect(r, full)));
        if region.is_empty() {
            return;
        }
        self.frame_serial += 1;
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(Entry {
            serial: self.frame_serial,
            rects: region.take(),
        });
    }

    pub(crate) fn frame_serial(&self) -> u64 {
        self.frame_serial
    }

    pub(crate) fn size_serial(&self) -> u64 {
        self.size_serial
    }

    pub(crate) fn cursor_serial(&self) -> u64 {
        self.cursor_serial
    }

    pub(crate) fn clipboard_serial(&self) -> u64 {
        self.clipboard_serial
    }

    /// Damage recorded after `serial`.
    pub(crate) fn changes_since(&self, serial: u64) -> Changes {
        if serial >= self.frame_serial {
            return Changes::Rects(Vec::new());
        }
        match self.history.front() {
            Some(oldest) if oldest.serial <= serial + 1 => Changes::Rects(
                self.history
                    .iter()
                    .filter(|e| e.serial > serial)
                    .flat_map(|e| e.rects.iter().copied())
                    .collect(),
            ),
            _ => Changes::Everything,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_damage_when_the_adapter_cannot() {
        let mut d = Desktop::new(128, 64, "d");
        let mut frame = d.framebuffer().clone();
        frame.fill(Rect::new(70, 3, 2, 2), 0xff).unwrap();
        d.update(frame, None);
        assert_eq!(
            d.changes_since(0),
            Changes::Rects(vec![Rect::new(64, 0, 64, 64)])
        );
        assert_eq!(d.changes_since(1), Changes::Rects(vec![]));
        // No change, no new serial.
        d.update(d.framebuffer().clone(), None);
        assert_eq!(d.frame_serial(), 1);
    }

    #[test]
    fn explicit_damage_is_clipped_and_regions_diffed() {
        let mut d = Desktop::new(100, 100, "d");
        d.update(d.framebuffer().clone(), Some(&[Rect::new(90, 90, 50, 50)]));
        assert_eq!(
            d.changes_since(0),
            Changes::Rects(vec![Rect::new(90, 90, 10, 10)])
        );
        d.update_region(Rect::new(0, 0, 2, 1), &[1, 0], None)
            .unwrap();
        assert_eq!(
            d.changes_since(1),
            Changes::Rects(vec![Rect::new(0, 0, 2, 1)])
        );
        assert!(d.update_region(Rect::new(0, 0, 2, 1), &[1], None).is_err());
        assert!(
            d.update_region(Rect::new(99, 0, 2, 1), &[1, 1], None)
                .is_err()
        );
    }

    #[test]
    fn falling_behind_the_history_means_everything() {
        let mut d = Desktop::new(10, 10, "d");
        for i in 0..=u32::try_from(HISTORY).unwrap() {
            d.update_region(Rect::new(0, 0, 1, 1), &[i + 1], None)
                .unwrap();
        }
        assert_eq!(d.changes_since(0), Changes::Everything);
        assert!(matches!(d.changes_since(2), Changes::Rects(_)));
    }

    #[test]
    fn resize_bumps_the_size_serial_and_damages_everything() {
        let mut d = Desktop::new(10, 10, "d");
        d.resize(20, 5);
        assert_eq!(d.size_serial(), 1);
        assert_eq!(
            d.changes_since(0),
            Changes::Rects(vec![Rect::new(0, 0, 20, 5)])
        );
        assert_eq!(d.screens()[0].width, 20);
    }
}
