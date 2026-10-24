//! The `ExtendedDesktopSize` pseudo-encoding (-308): framebuffer size plus
//! screen layout, and the outcome of `SetDesktopSize` requests.

use farsight_rfb::wire::Reader;
use farsight_rfb::{DecodeError, RectHeader, Screen};

use crate::util::need;

/// Why the server sent an `ExtendedDesktopSize` rectangle (its x-position).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResizeReason {
    /// Changed on the server, or the reply to a full update request.
    Server,
    /// The reply to this viewer's `SetDesktopSize`.
    ThisClient,
    /// Another viewer changed the layout.
    OtherClient,
    /// A reason this version does not know; treated like [`Self::Server`].
    Unknown(u16),
}

impl ResizeReason {
    fn from_code(code: u16) -> Self {
        match code {
            0 => Self::Server,
            1 => Self::ThisClient,
            2 => Self::OtherClient,
            other => Self::Unknown(other),
        }
    }
}

/// The result of this viewer's `SetDesktopSize` request (the y-position).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResizeStatus {
    /// The change was made.
    Ok,
    /// Resizing is administratively prohibited.
    Prohibited,
    /// The server is out of resources.
    OutOfResources,
    /// The requested screen layout is invalid.
    InvalidLayout,
    /// The request was forwarded and may complete later.
    Forwarded,
    /// An error code this version does not know.
    Unknown(u16),
}

impl ResizeStatus {
    fn from_code(code: u16) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::Prohibited,
            2 => Self::OutOfResources,
            3 => Self::InvalidLayout,
            4 => Self::Forwarded,
            other => Self::Unknown(other),
        }
    }
}

/// What the last `ExtendedDesktopSize` rectangle reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DesktopResize {
    /// Why it was sent.
    pub reason: ResizeReason,
    /// Outcome; only meaningful when `reason` is [`ResizeReason::ThisClient`]
    /// (the spec says to ignore it otherwise, so it is reported as `Ok`).
    pub status: ResizeStatus,
}

impl DesktopResize {
    fn from_codes(reason: u16, status: u16) -> Self {
        let reason = ResizeReason::from_code(reason);
        let status = if reason == ResizeReason::ThisClient {
            ResizeStatus::from_code(status)
        } else {
            ResizeStatus::Ok
        };
        Self { reason, status }
    }

    /// Whether the size and layout in the rectangle took effect. When this
    /// viewer's request failed, the remaining fields are undefined.
    #[must_use]
    pub fn applied(&self) -> bool {
        self.status == ResizeStatus::Ok
    }

    /// Whether this viewer's own request was refused or deferred.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        !self.applied()
    }
}

/// A parsed `ExtendedDesktopSize` rectangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtendedDesktopSize {
    pub(crate) result: DesktopResize,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) screens: Vec<Screen>,
}

/// Parses the payload: u8 screen count, 3 bytes padding, 16 bytes per screen.
pub(crate) fn parse(
    header: &RectHeader,
    input: &[u8],
) -> Result<Option<(ExtendedDesktopSize, usize)>, DecodeError> {
    let mut r = Reader::new(input);
    let count = need!(r.u8());
    need!(r.skip(3));
    let mut screens = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        screens.push(need!(Screen::decode(&mut r)));
    }
    let rect = header.rect;
    let parsed = ExtendedDesktopSize {
        result: DesktopResize::from_codes(rect.x, rect.y),
        width: rect.width,
        height: rect.height,
        screens,
    };
    Ok(Some((parsed, r.consumed())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use farsight_rfb::{Encoding, Rect};

    fn header(reason: u16, status: u16, w: u16, h: u16) -> RectHeader {
        RectHeader {
            rect: Rect::new(reason, status, w, h),
            encoding: Encoding::ExtendedDesktopSize,
        }
    }

    #[test]
    fn parses_screens_and_waits_for_all_of_them() {
        let screen = Screen {
            id: 9,
            x: 0,
            y: 0,
            width: 800,
            height: 600,
            flags: 0,
        };
        let mut payload = vec![1, 0, 0, 0];
        screen.encode(&mut payload);
        let h = header(0, 0, 800, 600);
        let (parsed, used) = parse(&h, &payload).unwrap().unwrap();
        assert_eq!(used, 20);
        assert_eq!(parsed.screens, [screen]);
        assert!(parsed.result.applied());
        for cut in 0..payload.len() {
            assert_eq!(parse(&h, &payload[..cut]), Ok(None));
        }
    }

    #[test]
    fn status_matters_only_for_this_clients_requests() {
        let failed = DesktopResize::from_codes(1, 3);
        assert_eq!(failed.reason, ResizeReason::ThisClient);
        assert_eq!(failed.status, ResizeStatus::InvalidLayout);
        assert!(failed.is_failure());
        assert!(DesktopResize::from_codes(0, 3).applied());
        assert!(DesktopResize::from_codes(2, 1).applied());
        assert_eq!(
            DesktopResize::from_codes(7, 0).reason,
            ResizeReason::Unknown(7)
        );
        assert_eq!(
            DesktopResize::from_codes(1, 99).status,
            ResizeStatus::Unknown(99)
        );
        assert!(DesktopResize::from_codes(1, 4).is_failure());
    }
}
