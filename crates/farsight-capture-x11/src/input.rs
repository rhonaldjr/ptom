//! The input thread: XTest key and pointer injection.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, KeyButMask, MOTION_NOTIFY_EVENT, Window,
};
use x11rb::protocol::xtest::{self, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use crate::buttons::{ButtonChange, button_changes};
use crate::keys::{
    KeyContext, Keymap, Level, ModChange, Modifier, XK_ISO_LEVEL3_SHIFT, XK_SHIFT_L, plan_modifiers,
};
use crate::{CaptureError, InputCommand};

/// A key the client holds down.
#[derive(Debug, Clone, Copy)]
struct HeldKey {
    keycode: u8,
    /// Whether `keycode` was temporarily remapped and must be restored on release.
    remapped: bool,
}

/// Which keycodes act as Shift and as `AltGr` (`ISO_Level3_Shift`).
#[derive(Debug, Default)]
struct ModifierKeys {
    shift_keycodes: Vec<u8>,
    level3_keycodes: Vec<u8>,
    level3_mask: u16,
}

/// Owns the input connection.
pub(crate) struct Injector {
    conn: RustConnection,
    root: Window,
    keymap: Keymap,
    modifiers: ModifierKeys,
    held: HashMap<u32, HeldKey>,
    buttons: u8,
}

impl Injector {
    /// Opens the display and checks for XTest.
    ///
    /// # Errors
    /// [`CaptureError::MissingExtension`] without XTest, or a connection error.
    pub(crate) fn connect(display: Option<&str>) -> Result<Self, CaptureError> {
        let (conn, screen_num) = x11rb::connect(display)?;
        let root = conn
            .setup()
            .roots
            .get(screen_num)
            .map(|s| s.root)
            .ok_or_else(|| CaptureError::UnsupportedVisual("screen not found".into()))?;
        if conn
            .extension_information(xtest::X11_EXTENSION_NAME)?
            .is_none()
        {
            return Err(CaptureError::MissingExtension("XTEST"));
        }
        conn.xtest_get_version(2, 2)?.reply()?;
        let mut injector = Self {
            conn,
            root,
            keymap: Keymap::default(),
            modifiers: ModifierKeys::default(),
            held: HashMap::new(),
            buttons: 0,
        };
        injector.load_mapping()?;
        Ok(injector)
    }

    /// Handles commands until every [`crate::InputHandle`] is dropped or the
    /// connection fails, then releases anything still held.
    pub(crate) fn run(mut self, commands: &Receiver<InputCommand>) {
        for command in commands {
            match self.handle(command) {
                Ok(()) => {}
                Err(err) if err.is_fatal() => {
                    tracing::warn!(%err, "X11 input stopped");
                    return;
                }
                Err(err) => tracing::debug!(%err, "input request failed"),
            }
        }
        if let Err(err) = self.release_all() {
            tracing::debug!(%err, "could not release held input");
        }
    }

    fn handle(&mut self, command: InputCommand) -> Result<(), CaptureError> {
        self.reload_mapping_if_changed()?;
        match command {
            InputCommand::Key { down: true, keysym } => self.key_down(keysym)?,
            InputCommand::Key {
                down: false,
                keysym,
            } => self.key_up(keysym)?,
            InputCommand::Pointer { buttons, x, y } => self.pointer(buttons, x, y)?,
        }
        self.conn.flush()?;
        Ok(())
    }

    fn reload_mapping_if_changed(&mut self) -> Result<(), CaptureError> {
        let mut changed = false;
        while let Some(event) = self.conn.poll_for_event()? {
            changed |= matches!(event, Event::MappingNotify(_));
        }
        if changed {
            self.load_mapping()?;
        }
        Ok(())
    }

    fn load_mapping(&mut self) -> Result<(), CaptureError> {
        let setup = self.conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let count = max.saturating_sub(min).saturating_add(1);
        let reply = self.conn.get_keyboard_mapping(min, count)?.reply()?;
        self.keymap = Keymap::new(min, reply.keysyms_per_keycode, reply.keysyms);
        let modmap = self.conn.get_modifier_mapping()?.reply()?;
        self.modifiers = self.modifier_keys(&modmap.keycodes, modmap.keycodes_per_modifier());
        Ok(())
    }

    fn modifier_keys(&self, keycodes: &[u8], per_modifier: u8) -> ModifierKeys {
        let per_modifier = usize::from(per_modifier).max(1);
        let rows: Vec<&[u8]> = keycodes.chunks(per_modifier).collect();
        let nonzero = |row: &[u8]| row.iter().copied().filter(|&k| k != 0).collect::<Vec<_>>();
        let shift_keycodes = rows.first().map(|row| nonzero(row)).unwrap_or_default();
        let level3_keycodes: Vec<u8> = self
            .keymap
            .lookup(XK_ISO_LEVEL3_SHIFT)
            .map(|(keycode, _)| keycode)
            .into_iter()
            .collect();
        let level3_mask = rows
            .iter()
            .position(|row| row.iter().any(|k| level3_keycodes.contains(k)))
            .and_then(|index| u32::try_from(index).ok())
            .map_or(0, |index| 1u16 << index);
        ModifierKeys {
            shift_keycodes,
            level3_keycodes,
            level3_mask,
        }
    }

    fn fake(&self, kind: u8, detail: u8) -> Result<(), CaptureError> {
        self.conn
            .xtest_fake_input(kind, detail, CURRENT_TIME, NONE, 0, 0, 0)?;
        Ok(())
    }

    fn key_down(&mut self, keysym: u32) -> Result<(), CaptureError> {
        if let Some(held) = self.held.get(&keysym) {
            // Auto-repeat from the client: press the same key again.
            return self.fake(KEY_PRESS_EVENT, held.keycode);
        }
        let held = if let Some((keycode, level)) = self.keymap.lookup(keysym) {
            self.press_mapped(keycode, keysym, level)?
        } else if let Some(keycode) = self.remap(keysym)? {
            self.fake(KEY_PRESS_EVENT, keycode)?;
            HeldKey {
                keycode,
                remapped: true,
            }
        } else {
            tracing::debug!("no spare keycode for an unmapped keysym");
            return Ok(());
        };
        self.held.insert(keysym, held);
        Ok(())
    }

    /// Presses `keycode`, adjusting Shift and `AltGr` around it for `level`.
    fn press_mapped(
        &self,
        keycode: u8,
        keysym: u32,
        level: Level,
    ) -> Result<HeldKey, CaptureError> {
        let (shift_held, level3_held) = self.modifier_state()?;
        let ctx = KeyContext {
            keycode,
            keysym,
            level,
            shift_held,
            level3_held,
        };
        let undo = self.apply_plan(&plan_modifiers(&self.keymap, ctx))?;
        self.fake(KEY_PRESS_EVENT, keycode)?;
        self.undo_plan(&undo)?;
        Ok(HeldKey {
            keycode,
            remapped: false,
        })
    }

    fn key_up(&mut self, keysym: u32) -> Result<(), CaptureError> {
        let Some(held) = self.held.remove(&keysym) else {
            return Ok(());
        };
        self.fake(KEY_RELEASE_EVENT, held.keycode)?;
        if held.remapped {
            self.set_keysym(held.keycode, 0)?;
        }
        Ok(())
    }

    /// Maps `keysym` onto a spare keycode, returning that keycode.
    fn remap(&mut self, keysym: u32) -> Result<Option<u8>, CaptureError> {
        let Some(keycode) = self.keymap.spare_keycode() else {
            return Ok(None);
        };
        self.set_keysym(keycode, keysym)?;
        Ok(Some(keycode))
    }

    /// Sets `keycode`'s plain and shifted symbol to `keysym` (0 clears it).
    fn set_keysym(&mut self, keycode: u8, keysym: u32) -> Result<(), CaptureError> {
        let per = self.keymap.per_keycode();
        let mut syms = vec![0u32; usize::from(per)];
        for slot in syms.iter_mut().take(2) {
            *slot = keysym;
        }
        self.conn
            .change_keyboard_mapping(1, keycode, per, &syms)?
            .check()?;
        self.keymap.set(keycode, &syms);
        Ok(())
    }

    /// Whether Shift and `AltGr` are currently in effect on the server.
    fn modifier_state(&self) -> Result<(bool, bool), CaptureError> {
        let reply = self.conn.query_pointer(self.root)?.reply()?;
        let mask = u16::from(reply.mask);
        let shift = mask & u16::from(KeyButMask::SHIFT) != 0;
        let level3 = mask & self.modifiers.level3_mask != 0;
        Ok((shift, level3))
    }

    /// Carries out `plan`; returns the changes that undo it.
    fn apply_plan(&self, plan: &[ModChange]) -> Result<Vec<(u8, u8)>, CaptureError> {
        let mut undo = Vec::new();
        for change in plan {
            match *change {
                ModChange::Press(modifier) => {
                    if let Some(keycode) = self.modifier_keycode(modifier) {
                        self.fake(KEY_PRESS_EVENT, keycode)?;
                        undo.push((KEY_RELEASE_EVENT, keycode));
                    }
                }
                ModChange::Release(modifier) => {
                    for keycode in self.keycodes_down(self.modifier_keycodes(modifier))? {
                        self.fake(KEY_RELEASE_EVENT, keycode)?;
                        undo.push((KEY_PRESS_EVENT, keycode));
                    }
                }
            }
        }
        Ok(undo)
    }

    fn undo_plan(&self, undo: &[(u8, u8)]) -> Result<(), CaptureError> {
        for &(kind, keycode) in undo.iter().rev() {
            self.fake(kind, keycode)?;
        }
        Ok(())
    }

    fn modifier_keycodes(&self, modifier: Modifier) -> &[u8] {
        match modifier {
            Modifier::Shift => &self.modifiers.shift_keycodes,
            Modifier::Level3 => &self.modifiers.level3_keycodes,
        }
    }

    fn modifier_keycode(&self, modifier: Modifier) -> Option<u8> {
        let keysym = match modifier {
            Modifier::Shift => XK_SHIFT_L,
            Modifier::Level3 => XK_ISO_LEVEL3_SHIFT,
        };
        self.keymap
            .lookup(keysym)
            .map(|(keycode, _)| keycode)
            .or_else(|| self.modifier_keycodes(modifier).first().copied())
    }

    /// Those of `keycodes` that are currently pressed.
    fn keycodes_down(&self, keycodes: &[u8]) -> Result<Vec<u8>, CaptureError> {
        let keys = self.conn.query_keymap()?.reply()?.keys;
        Ok(keycodes
            .iter()
            .copied()
            .filter(|&k| {
                keys.get(usize::from(k / 8))
                    .is_some_and(|byte| byte & (1 << (k % 8)) != 0)
            })
            .collect())
    }

    fn pointer(&mut self, buttons: u8, x: u16, y: u16) -> Result<(), CaptureError> {
        let x = i16::try_from(x).unwrap_or(i16::MAX);
        let y = i16::try_from(y).unwrap_or(i16::MAX);
        self.conn
            .xtest_fake_input(MOTION_NOTIFY_EVENT, 0, CURRENT_TIME, self.root, x, y, 0)?;
        for change in button_changes(self.buttons, buttons) {
            match change {
                ButtonChange::Press(button) => self.fake(BUTTON_PRESS_EVENT, button)?,
                ButtonChange::Release(button) => self.fake(BUTTON_RELEASE_EVENT, button)?,
            }
        }
        self.buttons = buttons;
        Ok(())
    }

    /// Releases held keys and buttons and restores remapped keycodes.
    fn release_all(&mut self) -> Result<(), CaptureError> {
        let keysyms: Vec<u32> = self.held.keys().copied().collect();
        for keysym in keysyms {
            self.key_up(keysym)?;
        }
        for change in button_changes(self.buttons, 0) {
            if let ButtonChange::Release(button) = change {
                self.fake(BUTTON_RELEASE_EVENT, button)?;
            }
        }
        self.buttons = 0;
        self.conn.flush()?;
        Ok(())
    }
}
