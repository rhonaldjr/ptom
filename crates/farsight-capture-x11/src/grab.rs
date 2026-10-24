//! Reading pixels from the root window, over MIT-SHM when possible.

use std::fs::File;
use std::os::unix::fs::FileExt;

use farsight_rfb::Rect;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ConnectionExt as _, ImageFormat, ImageOrder, Screen, VisualClass, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::CaptureError;
use crate::pixels::{PixelLayout, VisualFormat};

/// The pixel layout of `screen`'s root visual.
///
/// # Errors
/// [`CaptureError::UnsupportedVisual`] for anything but a TrueColor or
/// DirectColor visual with a pixmap format this backend understands.
pub(crate) fn root_layout(
    conn: &RustConnection,
    screen: &Screen,
) -> Result<PixelLayout, CaptureError> {
    let (depth, visual) = screen
        .allowed_depths
        .iter()
        .find_map(|d| {
            d.visuals
                .iter()
                .find(|v| v.visual_id == screen.root_visual)
                .map(|v| (d.depth, v))
        })
        .ok_or_else(|| CaptureError::UnsupportedVisual("root visual not listed".into()))?;
    if !matches!(
        visual.class,
        VisualClass::TRUE_COLOR | VisualClass::DIRECT_COLOR
    ) {
        return Err(CaptureError::UnsupportedVisual(format!(
            "visual class {:?} (only TrueColor/DirectColor are supported)",
            visual.class
        )));
    }
    let setup = conn.setup();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|f| f.depth == depth)
        .ok_or_else(|| {
            CaptureError::UnsupportedVisual(format!("no pixmap format for depth {depth}"))
        })?;
    PixelLayout::new(VisualFormat {
        bits_per_pixel: format.bits_per_pixel,
        scanline_pad: format.scanline_pad,
        big_endian: setup.image_byte_order == ImageOrder::MSB_FIRST,
        red_mask: visual.red_mask,
        green_mask: visual.green_mask,
        blue_mask: visual.blue_mask,
    })
}

/// A shared-memory segment created by the server (MIT-SHM 1.2) and read
/// through its file descriptor, so no `mmap` (and no `unsafe`) is needed.
#[derive(Debug)]
struct Segment {
    id: shm::Seg,
    file: File,
    size: usize,
}

/// Reads rectangles of the root window as `0x00RRGGBB` pixels.
#[derive(Debug)]
pub(crate) struct Grabber {
    root: Window,
    layout: PixelLayout,
    segment: Option<Segment>,
    shm_usable: bool,
}

impl Grabber {
    pub(crate) fn new(
        conn: &RustConnection,
        root: Window,
        layout: PixelLayout,
        width: u16,
        height: u16,
    ) -> Self {
        let mut grabber = Self {
            root,
            layout,
            segment: None,
            shm_usable: shm_supported(conn),
        };
        grabber.resize(conn, width, height);
        grabber
    }

    /// Whether grabs go through MIT-SHM.
    pub(crate) fn uses_shm(&self) -> bool {
        self.segment.is_some()
    }

    /// (Re)creates the shared segment for a `width` × `height` screen.
    pub(crate) fn resize(&mut self, conn: &RustConnection, width: u16, height: u16) {
        if let Some(old) = self.segment.take() {
            let _ = conn.shm_detach(old.id);
        }
        if !self.shm_usable {
            return;
        }
        match create_segment(conn, self.layout.stride(width) * usize::from(height)) {
            Ok(segment) => self.segment = Some(segment),
            Err(err) => {
                tracing::info!(%err, "MIT-SHM unavailable; using GetImage");
                self.shm_usable = false;
            }
        }
    }

    /// Reads `rect` of the root window.
    ///
    /// # Errors
    /// [`CaptureError`] if the X server refuses both the shared-memory and
    /// the plain `GetImage` request.
    pub(crate) fn grab(
        &mut self,
        conn: &RustConnection,
        rect: Rect,
    ) -> Result<Vec<u32>, CaptureError> {
        if let Some(segment) = &self.segment {
            match self.grab_shm(conn, segment, rect) {
                Ok(pixels) => return Ok(pixels),
                Err(err) => {
                    tracing::info!(%err, "MIT-SHM grab failed; falling back to GetImage");
                    let _ = conn.shm_detach(segment.id);
                    self.segment = None;
                    self.shm_usable = false;
                }
            }
        }
        self.grab_wire(conn, rect)
    }

    fn grab_shm(
        &self,
        conn: &RustConnection,
        segment: &Segment,
        rect: Rect,
    ) -> Result<Vec<u32>, CaptureError> {
        let (x, y) = origin(rect)?;
        let stride = self.layout.stride(rect.width);
        let len = stride * usize::from(rect.height);
        if len > segment.size {
            return Err(CaptureError::UnsupportedVisual(
                "grab larger than segment".into(),
            ));
        }
        conn.shm_get_image(
            self.root,
            x,
            y,
            rect.width,
            rect.height,
            u32::MAX,
            ImageFormat::Z_PIXMAP.into(),
            segment.id,
            0,
        )?
        .reply()?;
        let mut data = vec![0u8; len];
        segment.file.read_exact_at(&mut data, 0)?;
        Ok(self.layout.convert(&data, rect.width, rect.height, stride))
    }

    fn grab_wire(&self, conn: &RustConnection, rect: Rect) -> Result<Vec<u32>, CaptureError> {
        let (x, y) = origin(rect)?;
        let reply = conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                x,
                y,
                rect.width,
                rect.height,
                u32::MAX,
            )?
            .reply()?;
        let stride = self.layout.stride(rect.width);
        Ok(self
            .layout
            .convert(&reply.data, rect.width, rect.height, stride))
    }
}

fn origin(rect: Rect) -> Result<(i16, i16), CaptureError> {
    let too_big = |_| CaptureError::UnsupportedVisual("screen larger than 32767 pixels".into());
    Ok((
        i16::try_from(rect.x).map_err(too_big)?,
        i16::try_from(rect.y).map_err(too_big)?,
    ))
}

fn shm_supported(conn: &RustConnection) -> bool {
    let present = matches!(
        conn.extension_information(shm::X11_EXTENSION_NAME),
        Ok(Some(_))
    );
    present
        && conn
            .shm_query_version()
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|v| (v.major_version, v.minor_version) >= (1, 2))
}

fn create_segment(conn: &RustConnection, size: usize) -> Result<Segment, CaptureError> {
    let size_u32 = u32::try_from(size)
        .map_err(|_| CaptureError::UnsupportedVisual("screen too large for MIT-SHM".into()))?;
    let id = conn.generate_id()?;
    let reply = conn
        .shm_create_segment(id, size_u32.max(1), false)?
        .reply()?;
    Ok(Segment {
        id,
        file: File::from(reply.shm_fd),
        size,
    })
}
