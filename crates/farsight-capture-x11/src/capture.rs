//! The capture thread: damage tracking, frame pacing, resizes and cursor shapes.

use std::time::{Duration, Instant};

use farsight_codec::Framebuffer;
use farsight_rfb::Rect;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;
use x11rb::NONE;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, EventMask, Window};
use x11rb::rust_connection::RustConnection;

use crate::grab::{Grabber, root_layout};
use crate::pixels::cursor_to_rgba;
use crate::region::{changed_tiles, clip, coalesce};
use crate::{CaptureError, CaptureEvent, CursorShape, Frame, wait};

/// Minimum time between frames (about 30 per second).
const FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// Longest single wait, so a dropped receiver is noticed promptly.
const MAX_WAIT: Duration = Duration::from_millis(250);

/// Damage not yet delivered to the receiver.
#[derive(Debug)]
enum Pending {
    Full,
    Rects(Vec<Rect>),
}

impl Pending {
    fn add(this: Option<Self>, rects: Vec<Rect>) -> Self {
        match this {
            Some(Self::Full) => Self::Full,
            Some(Self::Rects(mut old)) => {
                old.extend(rects);
                Self::Rects(coalesce(&old))
            }
            None => Self::Rects(coalesce(&rects)),
        }
    }
}

/// XDamage objects tracking the root window.
#[derive(Debug)]
struct DamageTracker {
    damage: damage::Damage,
    region: xfixes::Region,
}

/// Owns the capture connection and the framebuffer.
#[allow(clippy::struct_excessive_bools)] // independent "work to do" flags
pub(crate) struct Capturer {
    conn: RustConnection,
    root: Window,
    framebuffer: Framebuffer,
    grabber: Grabber,
    damage: Option<DamageTracker>,
    has_xfixes: bool,
    poll_interval: Duration,
    pending: Option<Pending>,
    dirty: bool,
    resized: bool,
    full_refresh: bool,
    cursor_changed: bool,
    cursor_serial: Option<u32>,
    next_frame: Instant,
    next_poll: Instant,
}

impl Capturer {
    /// Opens the display and sets up damage, cursor and resize notifications.
    ///
    /// # Errors
    /// [`CaptureError`] if the display cannot be opened or its visual is unsupported.
    pub(crate) fn connect(
        display: Option<&str>,
        poll_interval: Duration,
    ) -> Result<Self, CaptureError> {
        let (conn, screen_num) = x11rb::connect(display)?;
        let screen = conn
            .setup()
            .roots
            .get(screen_num)
            .cloned()
            .ok_or_else(|| CaptureError::UnsupportedVisual("screen not found".into()))?;
        let layout = root_layout(&conn, &screen)?;
        let root = screen.root;
        let (width, height) = (screen.width_in_pixels, screen.height_in_pixels);

        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
        )?;
        let has_xfixes = init_xfixes(&conn, root);
        let damage = if has_xfixes {
            init_damage(&conn, root)
        } else {
            None
        };
        init_randr(&conn, root);
        if damage.is_none() {
            tracing::info!("XDamage unavailable; polling for changes");
        }

        let grabber = Grabber::new(&conn, root, layout, width, height);
        tracing::info!(
            width,
            height,
            shm = grabber.uses_shm(),
            damage = damage.is_some(),
            "X11 capture ready"
        );
        let now = Instant::now();
        Ok(Self {
            conn,
            root,
            framebuffer: Framebuffer::new(width, height),
            grabber,
            damage,
            has_xfixes,
            poll_interval: poll_interval.max(Duration::from_millis(10)),
            pending: None,
            dirty: false,
            resized: false,
            full_refresh: true,
            cursor_changed: has_xfixes,
            cursor_serial: None,
            next_frame: now,
            next_poll: now,
        })
    }

    /// Runs until the receiver is dropped or the X connection fails.
    pub(crate) fn run(mut self, events: &Sender<CaptureEvent>) {
        while !events.is_closed() {
            match self.step(events) {
                Ok(()) => {}
                Err(err) if err.is_fatal() => {
                    let reason = err.to_string();
                    tracing::warn!(%reason, "X11 capture stopped");
                    let _ = events.blocking_send(CaptureEvent::Closed(reason));
                    return;
                }
                Err(err) => {
                    // Typically a grab racing a resize: re-read the size and
                    // send a full frame after a short pause.
                    tracing::debug!(%err, "capture request failed");
                    self.resized = true;
                    self.full_refresh = true;
                    let _ = wait::readable(&[], FRAME_INTERVAL);
                }
            }
        }
    }

    fn step(&mut self, events: &Sender<CaptureEvent>) -> Result<(), CaptureError> {
        self.drain_events()?;
        if self.resized {
            self.resized = false;
            self.reload_size()?;
        }
        if self.full_refresh {
            self.full_refresh = false;
            self.collect_full()?;
        }
        if self.cursor_changed {
            self.cursor_changed = false;
            self.send_cursor(events)?;
        }
        let now = Instant::now();
        if now >= self.next_frame {
            if self.damage.is_some() && self.dirty {
                self.dirty = false;
                self.collect_damage()?;
            } else if self.damage.is_none() && now >= self.next_poll {
                self.next_poll = now + self.poll_interval;
                self.collect_poll()?;
            }
            if self.pending.is_some() {
                self.send_frame(events);
            }
        }
        self.conn.flush()?;
        let timeout = self
            .next_deadline()
            .saturating_duration_since(Instant::now());
        wait::readable(&[self.conn.stream()], timeout.min(MAX_WAIT))?;
        Ok(())
    }

    fn next_deadline(&self) -> Instant {
        let now = Instant::now();
        let wants_frame = self.pending.is_some() || self.dirty;
        match (wants_frame, self.damage.is_some()) {
            (true, _) => self.next_frame.max(now),
            (false, false) => self.next_poll.max(self.next_frame),
            (false, true) => now + MAX_WAIT,
        }
    }

    fn drain_events(&mut self) -> Result<(), CaptureError> {
        while let Some(event) = self.conn.poll_for_event()? {
            match event {
                Event::DamageNotify(_) => self.dirty = true,
                Event::XfixesCursorNotify(_) => self.cursor_changed = true,
                Event::RandrScreenChangeNotify(_) => self.resized = true,
                Event::ConfigureNotify(e) if e.window == self.root => self.resized = true,
                Event::Error(err) => tracing::debug!(?err, "X error on capture connection"),
                _ => {}
            }
        }
        Ok(())
    }

    /// Picks up a new root size; on change, the next frame is a full one.
    fn reload_size(&mut self) -> Result<(), CaptureError> {
        let geometry = self.conn.get_geometry(self.root)?.reply()?;
        let (width, height) = (geometry.width, geometry.height);
        if width != self.framebuffer.width() || height != self.framebuffer.height() {
            tracing::info!(width, height, "X screen resized");
            self.framebuffer.resize(width, height);
            self.grabber.resize(&self.conn, width, height);
            self.full_refresh = true;
        }
        Ok(())
    }

    /// Grabs the whole screen and marks it all as changed.
    fn collect_full(&mut self) -> Result<(), CaptureError> {
        if let Some(tracker) = &self.damage {
            // Everything is about to be grabbed; forget older damage.
            self.conn.damage_subtract(tracker.damage, NONE, NONE)?;
            self.dirty = false;
        }
        let full = Rect::new(0, 0, self.framebuffer.width(), self.framebuffer.height());
        self.refresh(&[full])?;
        self.pending = Some(Pending::Full);
        Ok(())
    }

    fn collect_damage(&mut self) -> Result<(), CaptureError> {
        let Some(tracker) = &self.damage else {
            return Ok(());
        };
        self.conn
            .damage_subtract(tracker.damage, NONE, tracker.region)?;
        let reply = self.conn.xfixes_fetch_region(tracker.region)?.reply()?;
        let (width, height) = (self.framebuffer.width(), self.framebuffer.height());
        let rects: Vec<Rect> = reply
            .rectangles
            .iter()
            .filter_map(|r| clip(r.x, r.y, r.width, r.height, width, height))
            .collect();
        let rects = coalesce(&rects);
        if rects.is_empty() {
            return Ok(());
        }
        self.refresh(&rects)?;
        self.pending = Some(Pending::add(self.pending.take(), rects));
        Ok(())
    }

    fn collect_poll(&mut self) -> Result<(), CaptureError> {
        let (width, height) = (self.framebuffer.width(), self.framebuffer.height());
        let full = Rect::new(0, 0, width, height);
        let pixels = self.grabber.grab(&self.conn, full)?;
        let changed = changed_tiles(self.framebuffer.pixels(), &pixels, width, height);
        if changed.is_empty() {
            return Ok(());
        }
        write_pixels(&mut self.framebuffer, full, &pixels);
        self.pending = Some(Pending::add(self.pending.take(), changed));
        Ok(())
    }

    /// Grabs `rects` into the framebuffer.
    fn refresh(&mut self, rects: &[Rect]) -> Result<(), CaptureError> {
        for &rect in rects {
            let pixels = self.grabber.grab(&self.conn, rect)?;
            write_pixels(&mut self.framebuffer, rect, &pixels);
        }
        Ok(())
    }

    /// Sends pending damage; on a full channel keeps it for the next frame.
    fn send_frame(&mut self, events: &Sender<CaptureEvent>) {
        let damage = match &self.pending {
            Some(Pending::Rects(rects)) => Some(rects.clone()),
            Some(Pending::Full) | None => None,
        };
        let frame = Frame {
            framebuffer: self.framebuffer.clone(),
            damage,
        };
        match events.try_send(CaptureEvent::Frame(frame)) {
            Ok(()) => {
                self.pending = None;
                self.next_frame = Instant::now() + FRAME_INTERVAL;
            }
            Err(TrySendError::Full(_)) => self.next_frame = Instant::now() + FRAME_INTERVAL,
            Err(TrySendError::Closed(_)) => {}
        }
    }

    fn send_cursor(&mut self, events: &Sender<CaptureEvent>) -> Result<(), CaptureError> {
        if !self.has_xfixes {
            return Ok(());
        }
        let image = self.conn.xfixes_get_cursor_image()?.reply()?;
        if self.cursor_serial == Some(image.cursor_serial) {
            return Ok(());
        }
        self.cursor_serial = Some(image.cursor_serial);
        let pixel_count = usize::from(image.width) * usize::from(image.height);
        let shape =
            (pixel_count > 0 && image.cursor_image.len() >= pixel_count).then(|| CursorShape {
                width: image.width,
                height: image.height,
                hotspot: (image.xhot, image.yhot),
                rgba: cursor_to_rgba(image.cursor_image.get(..pixel_count).unwrap_or_default()),
            });
        let _ = events.blocking_send(CaptureEvent::Cursor(shape));
        Ok(())
    }
}

fn write_pixels(framebuffer: &mut Framebuffer, rect: Rect, pixels: &[u32]) {
    let result = framebuffer.write(rect, |i| pixels.get(i).copied().unwrap_or(0));
    if result.is_err() {
        tracing::debug!(?rect, "grabbed area outside framebuffer; skipped");
    }
}

fn init_xfixes(conn: &RustConnection, root: Window) -> bool {
    if !matches!(
        conn.extension_information(xfixes::X11_EXTENSION_NAME),
        Ok(Some(_))
    ) {
        tracing::info!("XFixes unavailable; no cursor shapes");
        return false;
    }
    let ok = conn
        .xfixes_query_version(5, 0)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some_and(|v| v.major_version >= 2);
    if ok {
        let _ = conn.xfixes_select_cursor_input(root, xfixes::CursorNotifyMask::DISPLAY_CURSOR);
    }
    ok
}

fn init_damage(conn: &RustConnection, root: Window) -> Option<DamageTracker> {
    if !matches!(
        conn.extension_information(damage::X11_EXTENSION_NAME),
        Ok(Some(_))
    ) {
        return None;
    }
    conn.damage_query_version(1, 1).ok()?.reply().ok()?;
    let damage = conn.generate_id().ok()?;
    let region = conn.generate_id().ok()?;
    conn.damage_create(damage, root, damage::ReportLevel::NON_EMPTY)
        .ok()?
        .check()
        .ok()?;
    conn.xfixes_create_region(region, &[]).ok()?.check().ok()?;
    Some(DamageTracker { damage, region })
}

fn init_randr(conn: &RustConnection, root: Window) {
    if !matches!(
        conn.extension_information(randr::X11_EXTENSION_NAME),
        Ok(Some(_))
    ) {
        return;
    }
    let ok = conn
        .randr_query_version(1, 2)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some();
    if ok {
        let _ = conn.randr_select_input(root, randr::NotifyMask::SCREEN_CHANGE);
    }
}
