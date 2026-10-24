//! Turning the X11 keysyms viewers send into key presses on the
//! compositor's keyboard (US layout, the same keymap the seat uses).

use std::collections::HashMap;

use smithay::input::keyboard::xkb;

/// Shift's evdev keycode plus the xkb offset of 8 (`KEY_LEFTSHIFT`).
pub(crate) const SHIFT: u32 = 42 + 8;
/// Both Shift keys (`KEY_LEFTSHIFT`, `KEY_RIGHTSHIFT`, plus 8).
pub(crate) const SHIFT_KEYS: [u32; 2] = [SHIFT, 54 + 8];

/// Shift presses or releases to wrap around a key so it produces the
/// keysym the viewer meant: `(keycode, pressed)` before and after it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ShiftFix {
    pub before: Vec<(u32, bool)>,
    pub after: Vec<(u32, bool)>,
}

/// What to do around a key press given whether its keysym needs Shift and
/// which Shift keys are held. Releases need nothing.
pub(crate) fn shift_fix(down: bool, needs_shift: bool, held_shifts: &[u32]) -> ShiftFix {
    if !down {
        return ShiftFix::default();
    }
    match (needs_shift, held_shifts.is_empty()) {
        (true, true) => ShiftFix {
            before: vec![(SHIFT, true)],
            after: vec![(SHIFT, false)],
        },
        (false, false) => ShiftFix {
            before: held_shifts.iter().map(|&k| (k, false)).collect(),
            after: held_shifts.iter().map(|&k| (k, true)).collect(),
        },
        _ => ShiftFix::default(),
    }
}

/// Where a keysym lives on the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyPosition {
    /// xkb keycode.
    pub keycode: u32,
    /// Whether the keysym needs Shift (it's on the second level).
    pub shifted: bool,
}

/// Keysym → key lookup built from the default keymap.
pub(crate) struct KeyLookup {
    keys: HashMap<u32, KeyPosition>,
}

impl KeyLookup {
    /// Builds the table from the default (US) keymap. `None` if xkbcommon
    /// has no keymap data installed.
    pub fn new() -> Option<Self> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        let mut keys = HashMap::new();
        keymap.key_for_each(|keymap, keycode| {
            for level in 0..keymap.num_levels_for_key(keycode, 0).min(2) {
                for sym in keymap.key_get_syms_by_level(keycode, 0, level) {
                    // The first (lowest) position wins, e.g. the main row over the keypad.
                    keys.entry(sym.raw()).or_insert(KeyPosition {
                        keycode: keycode.raw(),
                        shifted: level == 1,
                    });
                }
            }
        });
        Some(Self { keys })
    }

    /// Where `keysym` is, if the keymap has it.
    pub fn find(&self, keysym: u32) -> Option<KeyPosition> {
        self.keys.get(&keysym).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_is_added_or_lifted_only_when_needed() {
        let right = SHIFT_KEYS[1];
        assert_eq!(
            shift_fix(true, true, &[]),
            ShiftFix {
                before: vec![(SHIFT, true)],
                after: vec![(SHIFT, false)]
            }
        );
        assert_eq!(
            shift_fix(true, false, &[right]),
            ShiftFix {
                before: vec![(right, false)],
                after: vec![(right, true)]
            }
        );
        assert_eq!(shift_fix(true, true, &[SHIFT]), ShiftFix::default());
        assert_eq!(shift_fix(true, false, &[]), ShiftFix::default());
        assert_eq!(shift_fix(false, true, &[]), ShiftFix::default());
    }

    #[test]
    fn letters_digits_and_shifted_symbols_are_found() {
        let Some(keys) = KeyLookup::new() else {
            eprintln!("no xkb data; skipping");
            return;
        };
        let a = keys.find(u32::from(b'a')).unwrap();
        let capital_a = keys.find(u32::from(b'A')).unwrap();
        assert_eq!(a.keycode, capital_a.keycode);
        assert!(!a.shifted && capital_a.shifted);
        assert_eq!(keys.find(u32::from(b'1')).unwrap().keycode, 2 + 8);
        assert!(keys.find(u32::from(b'!')).unwrap().shifted);
        // Return (0xff0d) and Shift_L (0xffe1).
        assert_eq!(keys.find(0xff0d).unwrap().keycode, 28 + 8);
        assert_eq!(keys.find(0xffe1).unwrap().keycode, SHIFT);
        assert!(
            keys.find(0x0100_263a).is_none(),
            "an emoji isn't on the keyboard"
        );
    }
}
