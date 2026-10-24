//! The clipboard between viewers, Wayland apps and X11 apps.
//!
//! - Viewer → desktop: the compositor owns the selection and answers
//!   requests from its copy of the text.
//! - App → viewer: when an app sets the clipboard, the compositor reads it
//!   (text only, at most [`MAX_CLIPBOARD_BYTES`]) and sends it to the agent.
//!
//! Transfers happen on short-lived threads so a slow app never stalls the
//! compositor. Clipboard contents are never logged.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::sync::Arc;

use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::selection::data_device::set_data_device_selection;
use tokio::sync::mpsc::Sender;
use tracing::debug;

use crate::state::{ClipboardOwner, State};
use crate::{HeadlessEvent, MAX_CLIPBOARD_BYTES};

/// Most clipboard transfers running at once. A transfer that an app never
/// finishes holds a thread; past this, new ones are dropped instead of piling up.
const MAX_TRANSFERS: usize = 8;
static TRANSFERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Runs `work` on its own thread if fewer than [`MAX_TRANSFERS`] are running.
fn transfer(work: impl FnOnce() + Send + 'static) {
    use std::sync::atomic::Ordering;
    if TRANSFERS.fetch_add(1, Ordering::AcqRel) >= MAX_TRANSFERS {
        TRANSFERS.fetch_sub(1, Ordering::AcqRel);
        debug!("too many clipboard transfers running; dropped one");
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("farsight-clipboard".into())
        .spawn(move || {
            work();
            TRANSFERS.fetch_sub(1, Ordering::AcqRel);
        });
    if spawned.is_err() {
        // Out of threads: skip this transfer rather than crash the desktop.
        TRANSFERS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Text types, most preferred first (Wayland names, then X11 ones).
const TEXT_TYPES: [&str; 4] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
];

/// The types the compositor offers for text it owns.
pub(crate) fn offered_types() -> Vec<String> {
    TEXT_TYPES.iter().map(|t| (*t).to_owned()).collect()
}

/// The best text type among `types`, if any.
pub(crate) fn text_type(types: &[String]) -> Option<String> {
    TEXT_TYPES
        .iter()
        .find(|wanted| types.iter().any(|t| t == *wanted))
        .map(|t| (*t).to_owned())
}

/// A viewer sent clipboard text: make it the desktop's clipboard, for
/// Wayland and X11 apps alike.
pub(crate) fn set_from_viewer(state: &mut State, text: String) {
    set_data_device_selection(
        &state.display_handle,
        &state.seat,
        offered_types(),
        ClipboardOwner::Viewer(Arc::from(text)),
    );
    if let Some(xwm) = state.xwm.as_mut()
        && let Err(error) = xwm.new_selection(SelectionTarget::Clipboard, Some(offered_types()))
    {
        debug!(%error, "could not offer the clipboard to X11 apps");
    }
}

/// Answers an app's request for text the compositor owns.
pub(crate) fn write_text(text: Arc<str>, fd: OwnedFd) {
    transfer(move || {
        let _ = File::from(fd).write_all(text.as_bytes());
    });
}

/// A pipe whose write end goes to the app that owns the clipboard; what it
/// writes is forwarded to the agent.
pub(crate) fn reader_for_viewer(events: Sender<HeadlessEvent>) -> std::io::Result<OwnedFd> {
    let (reader, writer) = std::io::pipe()?;
    transfer(move || {
        let mut text = Vec::new();
        let limit = u64::try_from(MAX_CLIPBOARD_BYTES + 1).unwrap_or(u64::MAX);
        if reader.take(limit).read_to_end(&mut text).is_err() || text.len() > MAX_CLIPBOARD_BYTES {
            return;
        }
        if let Ok(text) = String::from_utf8(text)
            && !text.is_empty()
        {
            let _ = events.blocking_send(HeadlessEvent::Clipboard(text));
        }
    });
    Ok(OwnedFd::from(writer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_preferred_text_type_wins() {
        let offered = vec![
            "image/png".to_owned(),
            "STRING".to_owned(),
            "UTF8_STRING".to_owned(),
        ];
        assert_eq!(text_type(&offered).as_deref(), Some("UTF8_STRING"));
        assert_eq!(text_type(&["image/png".to_owned()]), None);
    }

    #[test]
    fn text_from_an_app_reaches_the_viewer_and_oversized_text_does_not() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(2);
        let fd = reader_for_viewer(tx.clone()).unwrap();
        File::from(fd).write_all("héllo".as_bytes()).unwrap();
        assert!(matches!(
            runtime.block_on(rx.recv()),
            Some(HeadlessEvent::Clipboard(text)) if text == "héllo"
        ));
        let fd = reader_for_viewer(tx).unwrap();
        File::from(fd)
            .write_all(&vec![b'a'; MAX_CLIPBOARD_BYTES + 1])
            .unwrap();
        assert!(
            runtime.block_on(rx.recv()).is_none(),
            "too big: nothing is sent"
        );
    }
}
