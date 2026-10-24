//! The clipboard thread: serves `CLIPBOARD` for the remote side and reports
//! other clients' clipboard changes (ICCCM selections, XFixes notifications).

use std::os::unix::net::UnixStream;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use tokio::sync::mpsc::Sender;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

use crate::{CaptureError, CaptureEvent, MAX_CLIPBOARD_BYTES, wait};

/// Longest single wait, so a dropped receiver is noticed promptly.
const MAX_WAIT: Duration = Duration::from_millis(500);

/// Atoms used for selection transfers.
#[derive(Debug, Clone, Copy)]
struct Atoms {
    clipboard: Atom,
    utf8: Atom,
    targets: Atom,
    incr: Atom,
    /// Property on our window that incoming transfers are written to.
    transfer: Atom,
}

/// Progress of fetching another client's clipboard.
#[derive(Debug)]
enum Incoming {
    Idle,
    Requested {
        target: Atom,
    },
    Incremental {
        kind: Atom,
        data: Vec<u8>,
        overflow: bool,
    },
}

/// Owns the clipboard connection and its helper window.
pub(crate) struct ClipboardOwner {
    conn: RustConnection,
    window: Window,
    atoms: Atoms,
    wake: UnixStream,
    owned: Option<String>,
    incoming: Incoming,
}

impl ClipboardOwner {
    /// Opens the display and starts watching `CLIPBOARD`. Returns `None` when
    /// the server lacks XFixes (clipboard sync is then unavailable).
    ///
    /// # Errors
    /// [`CaptureError`] if the display cannot be opened or set-up requests fail.
    pub(crate) fn connect(
        display: Option<&str>,
        wake: UnixStream,
    ) -> Result<Option<Self>, CaptureError> {
        let (conn, screen_num) = x11rb::connect(display)?;
        if conn
            .extension_information(xfixes::X11_EXTENSION_NAME)?
            .is_none()
        {
            tracing::warn!("XFixes unavailable; clipboard sync disabled");
            return Ok(None);
        }
        conn.xfixes_query_version(5, 0)?.reply()?;
        let root = conn
            .setup()
            .roots
            .get(screen_num)
            .map(|s| s.root)
            .ok_or_else(|| CaptureError::UnsupportedVisual("screen not found".into()))?;
        let window = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            -1,
            -1,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?
        .check()?;
        let atoms = intern_atoms(&conn)?;
        conn.xfixes_select_selection_input(
            window,
            atoms.clipboard,
            xfixes::SelectionEventMask::SET_SELECTION_OWNER,
        )?
        .check()?;
        Ok(Some(Self {
            conn,
            window,
            atoms,
            wake,
            owned: None,
            incoming: Incoming::Idle,
        }))
    }

    /// Runs until the event receiver is dropped or the connection fails.
    pub(crate) fn run(mut self, events: &Sender<CaptureEvent>, texts: &Receiver<String>) {
        let mut wake_open = true;
        while !events.is_closed() {
            if let Err(err) = self.step(events, texts, &mut wake_open) {
                if err.is_fatal() {
                    tracing::warn!(%err, "X11 clipboard stopped");
                    return;
                }
                tracing::debug!(%err, "clipboard request failed");
            }
        }
    }

    fn step(
        &mut self,
        events: &Sender<CaptureEvent>,
        texts: &Receiver<String>,
        wake_open: &mut bool,
    ) -> Result<(), CaptureError> {
        while let Ok(text) = texts.try_recv() {
            self.take_ownership(text)?;
        }
        while let Some(event) = self.conn.poll_for_event()? {
            self.handle_event(event, events)?;
        }
        self.conn.flush()?;
        if *wake_open {
            let ready = wait::readable(&[self.conn.stream(), &self.wake], MAX_WAIT)?;
            if ready.get(1).copied().unwrap_or(false) {
                *wake_open = wait::drain(&self.wake);
            }
        } else {
            wait::readable(&[self.conn.stream()], MAX_WAIT)?;
        }
        Ok(())
    }

    fn take_ownership(&mut self, text: String) -> Result<(), CaptureError> {
        self.owned = Some(text);
        self.conn
            .set_selection_owner(self.window, self.atoms.clipboard, CURRENT_TIME)?;
        let owner = self
            .conn
            .get_selection_owner(self.atoms.clipboard)?
            .reply()?
            .owner;
        if owner != self.window {
            tracing::warn!("could not take the X clipboard");
            self.owned = None;
        }
        Ok(())
    }

    fn handle_event(
        &mut self,
        event: Event,
        events: &Sender<CaptureEvent>,
    ) -> Result<(), CaptureError> {
        match event {
            Event::XfixesSelectionNotify(e)
                if e.selection == self.atoms.clipboard
                    && e.owner != self.window
                    && e.owner != NONE =>
            {
                self.request(self.atoms.utf8, e.selection_timestamp)?;
            }
            Event::SelectionNotify(e)
                if e.requestor == self.window && e.selection == self.atoms.clipboard =>
            {
                self.on_selection_notify(e.property, e.time, events)?;
            }
            Event::PropertyNotify(e)
                if e.window == self.window
                    && e.atom == self.atoms.transfer
                    && e.state == Property::NEW_VALUE =>
            {
                self.on_incremental_chunk(events)?;
            }
            Event::SelectionRequest(e) => self.serve(&e)?,
            Event::SelectionClear(e) if e.selection == self.atoms.clipboard => self.owned = None,
            Event::Error(err) => tracing::debug!(?err, "X error on clipboard connection"),
            _ => {}
        }
        Ok(())
    }

    fn request(&mut self, target: Atom, time: u32) -> Result<(), CaptureError> {
        self.conn.convert_selection(
            self.window,
            self.atoms.clipboard,
            target,
            self.atoms.transfer,
            time,
        )?;
        self.incoming = Incoming::Requested { target };
        Ok(())
    }

    fn on_selection_notify(
        &mut self,
        property: Atom,
        time: u32,
        events: &Sender<CaptureEvent>,
    ) -> Result<(), CaptureError> {
        let Incoming::Requested { target } = self.incoming else {
            return Ok(());
        };
        if property == NONE {
            // The owner refused; fall back from UTF8_STRING to STRING once.
            self.incoming = Incoming::Idle;
            if target == self.atoms.utf8 {
                self.request(AtomEnum::STRING.into(), time)?;
            }
            return Ok(());
        }
        let reply = self.read_transfer()?;
        if reply.type_ == self.atoms.incr {
            // Deleting the property (done by the read) starts the transfer.
            self.incoming = Incoming::Incremental {
                kind: NONE,
                data: Vec::new(),
                overflow: false,
            };
            return Ok(());
        }
        self.incoming = Incoming::Idle;
        if reply.bytes_after > 0 {
            self.conn
                .delete_property(self.window, self.atoms.transfer)?;
            tracing::info!("clipboard text over 1 MiB; ignored");
            return Ok(());
        }
        emit(reply.type_, &reply.value, events);
        Ok(())
    }

    fn on_incremental_chunk(&mut self, events: &Sender<CaptureEvent>) -> Result<(), CaptureError> {
        if !matches!(self.incoming, Incoming::Incremental { .. }) {
            return Ok(());
        }
        let reply = self.read_transfer()?;
        let Incoming::Incremental {
            kind,
            data,
            overflow,
        } = &mut self.incoming
        else {
            return Ok(());
        };
        if reply.value.is_empty() {
            let (kind, data, overflow) = (*kind, std::mem::take(data), *overflow);
            self.incoming = Incoming::Idle;
            if overflow {
                tracing::info!("clipboard text over 1 MiB; ignored");
            } else {
                emit(kind, &data, events);
            }
            return Ok(());
        }
        *kind = reply.type_;
        if *overflow || data.len() + reply.value.len() > MAX_CLIPBOARD_BYTES {
            *overflow = true;
            data.clear();
        } else {
            data.extend_from_slice(&reply.value);
        }
        if reply.bytes_after > 0 {
            self.conn
                .delete_property(self.window, self.atoms.transfer)?;
        }
        Ok(())
    }

    fn read_transfer(&self) -> Result<x11rb::protocol::xproto::GetPropertyReply, CaptureError> {
        let longs = u32::try_from(MAX_CLIPBOARD_BYTES.div_ceil(4)).unwrap_or(u32::MAX);
        Ok(self
            .conn
            .get_property(
                true,
                self.window,
                self.atoms.transfer,
                AtomEnum::ANY,
                0,
                longs,
            )?
            .reply()?)
    }

    /// Answers another client's request for our clipboard.
    fn serve(&self, request: &SelectionRequestEvent) -> Result<(), CaptureError> {
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let served = match &self.owned {
            Some(text) if request.selection == self.atoms.clipboard => {
                self.write_target(request.requestor, property, request.target, text)?
            }
            _ => false,
        };
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if served { property } else { NONE },
        };
        self.conn
            .send_event(false, request.requestor, EventMask::NO_EVENT, notify)?;
        Ok(())
    }

    /// Writes `text` to `property` on `requestor` as `target`; `false` if
    /// the target is not offered or the text does not fit in one request.
    fn write_target(
        &self,
        requestor: Window,
        property: Atom,
        target: Atom,
        text: &str,
    ) -> Result<bool, CaptureError> {
        let string = Atom::from(AtomEnum::STRING);
        if target == self.atoms.targets {
            let offered = [self.atoms.targets, self.atoms.utf8, string];
            self.conn.change_property32(
                PropMode::REPLACE,
                requestor,
                property,
                AtomEnum::ATOM,
                &offered,
            )?;
            return Ok(true);
        }
        let bytes = if target == self.atoms.utf8 {
            text.as_bytes().to_vec()
        } else if target == string {
            latin1(text)
        } else {
            return Ok(false);
        };
        if bytes.len() + 64 > self.conn.maximum_request_bytes() {
            tracing::info!("clipboard text too large for one X request; refused");
            return Ok(false);
        }
        self.conn
            .change_property8(PropMode::REPLACE, requestor, property, target, &bytes)?;
        Ok(true)
    }
}

/// Decodes clipboard bytes of type `kind` and sends them on.
fn emit(kind: Atom, bytes: &[u8], events: &Sender<CaptureEvent>) {
    let text = if kind == Atom::from(AtomEnum::STRING) {
        bytes.iter().map(|&b| char::from(b)).collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let _ = events.blocking_send(CaptureEvent::Clipboard(text));
}

/// `text` as ISO 8859-1, with characters outside it replaced by `?`.
fn latin1(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

fn intern_atoms(conn: &RustConnection) -> Result<Atoms, CaptureError> {
    let intern = |name: &[u8]| -> Result<Atom, CaptureError> {
        Ok(conn.intern_atom(false, name)?.reply()?.atom)
    };
    Ok(Atoms {
        clipboard: intern(b"CLIPBOARD")?,
        utf8: intern(b"UTF8_STRING")?,
        targets: intern(b"TARGETS")?,
        incr: intern(b"INCR")?,
        transfer: intern(b"FARSIGHT_CLIPBOARD")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_replaces_wide_characters() {
        assert_eq!(latin1("aé€"), [b'a', 0xE9, b'?']);
    }
}
