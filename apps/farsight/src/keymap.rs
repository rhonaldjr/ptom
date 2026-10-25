//! Maps iced keyboard events to X11 keysyms.

use cosmic::iced::keyboard::key::Named;
use cosmic::iced::keyboard::key::{Code, NativeCode, Physical};
use cosmic::iced::keyboard::{Key, Location};
use farsight_client::keysym::{self, named as k};

/// The keysym for a key event, preferring the shifted character so the
/// server sees what the user typed (e.g. Shift+1 → `!`).
pub fn keysym(key: &Key, modified: &Key, location: Location) -> Option<u32> {
    match modified {
        Key::Character(text) => single_char(text).map(keysym::from_char),
        Key::Named(name) => named(*name, location),
        Key::Unidentified => match key {
            Key::Character(text) => single_char(text).map(keysym::from_char),
            Key::Named(name) => named(*name, location),
            Key::Unidentified => None,
        },
    }
}

fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

fn named(name: Named, location: Location) -> Option<u32> {
    let right = location == Location::Right;
    Some(match name {
        Named::Enter => k::RETURN,
        Named::Tab => k::TAB,
        Named::Backspace => k::BACKSPACE,
        Named::Escape => k::ESCAPE,
        Named::Delete => k::DELETE,
        Named::Insert => k::INSERT,
        Named::Home => k::HOME,
        Named::End => k::END,
        Named::PageUp => k::PAGE_UP,
        Named::PageDown => k::PAGE_DOWN,
        Named::ArrowLeft => k::LEFT,
        Named::ArrowUp => k::UP,
        Named::ArrowRight => k::RIGHT,
        Named::ArrowDown => k::DOWN,
        Named::Shift => {
            if right {
                k::SHIFT_R
            } else {
                k::SHIFT_L
            }
        }
        Named::Control => {
            if right {
                k::CONTROL_R
            } else {
                k::CONTROL_L
            }
        }
        Named::Alt => {
            if right {
                k::ALT_R
            } else {
                k::ALT_L
            }
        }
        Named::AltGraph => k::ISO_LEVEL3_SHIFT,
        Named::Super | Named::Meta => {
            if right {
                k::SUPER_R
            } else {
                k::SUPER_L
            }
        }
        Named::CapsLock => k::CAPS_LOCK,
        Named::NumLock => k::NUM_LOCK,
        Named::ScrollLock => k::SCROLL_LOCK,
        Named::PrintScreen => k::PRINT,
        Named::Pause => k::PAUSE,
        Named::ContextMenu => k::MENU,
        other => return function_key(other),
    })
}

fn function_key(name: Named) -> Option<u32> {
    let n = match name {
        Named::F1 => 1,
        Named::F2 => 2,
        Named::F3 => 3,
        Named::F4 => 4,
        Named::F5 => 5,
        Named::F6 => 6,
        Named::F7 => 7,
        Named::F8 => 8,
        Named::F9 => 9,
        Named::F10 => 10,
        Named::F11 => 11,
        Named::F12 => 12,
        _ => return None,
    };
    keysym::function_key(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_use_the_modified_key() {
        let key = Key::Character("1".into());
        let shifted = Key::Character("!".into());
        assert_eq!(keysym(&key, &shifted, Location::Standard), Some(0x21));
    }

    #[test]
    fn named_keys_respect_location() {
        let shift = Key::Named(Named::Shift);
        assert_eq!(keysym(&shift, &shift, Location::Left), Some(k::SHIFT_L));
        assert_eq!(keysym(&shift, &shift, Location::Right), Some(k::SHIFT_R));
        let f8 = Key::Named(Named::F8);
        assert_eq!(keysym(&f8, &f8, Location::Standard), Some(0xffc5));
    }
}

/// Physical keys and their Linux evdev codes (US keyboard positions).
const EVDEV: &[(Code, u16)] = &[
    (Code::KeyQ, 16),
    (Code::KeyW, 17),
    (Code::KeyE, 18),
    (Code::KeyR, 19),
    (Code::KeyT, 20),
    (Code::KeyY, 21),
    (Code::KeyU, 22),
    (Code::KeyI, 23),
    (Code::KeyO, 24),
    (Code::KeyP, 25),
    (Code::KeyA, 30),
    (Code::KeyS, 31),
    (Code::KeyD, 32),
    (Code::KeyF, 33),
    (Code::KeyG, 34),
    (Code::KeyH, 35),
    (Code::KeyJ, 36),
    (Code::KeyK, 37),
    (Code::KeyL, 38),
    (Code::KeyZ, 44),
    (Code::KeyX, 45),
    (Code::KeyC, 46),
    (Code::KeyV, 47),
    (Code::KeyB, 48),
    (Code::KeyN, 49),
    (Code::KeyM, 50),
    (Code::Escape, 1),
    (Code::Digit1, 2),
    (Code::Digit2, 3),
    (Code::Digit3, 4),
    (Code::Digit4, 5),
    (Code::Digit5, 6),
    (Code::Digit6, 7),
    (Code::Digit7, 8),
    (Code::Digit8, 9),
    (Code::Digit9, 10),
    (Code::Digit0, 11),
    (Code::Minus, 12),
    (Code::Equal, 13),
    (Code::Backspace, 14),
    (Code::Tab, 15),
    (Code::BracketLeft, 26),
    (Code::BracketRight, 27),
    (Code::Enter, 28),
    (Code::ControlLeft, 29),
    (Code::Semicolon, 39),
    (Code::Quote, 40),
    (Code::Backquote, 41),
    (Code::ShiftLeft, 42),
    (Code::Backslash, 43),
    (Code::Comma, 51),
    (Code::Period, 52),
    (Code::Slash, 53),
    (Code::ShiftRight, 54),
    (Code::NumpadMultiply, 55),
    (Code::AltLeft, 56),
    (Code::Space, 57),
    (Code::CapsLock, 58),
    (Code::F1, 59),
    (Code::F2, 60),
    (Code::F3, 61),
    (Code::F4, 62),
    (Code::F5, 63),
    (Code::F6, 64),
    (Code::F7, 65),
    (Code::F8, 66),
    (Code::F9, 67),
    (Code::F10, 68),
    (Code::NumLock, 69),
    (Code::ScrollLock, 70),
    (Code::Numpad7, 71),
    (Code::Numpad8, 72),
    (Code::Numpad9, 73),
    (Code::NumpadSubtract, 74),
    (Code::Numpad4, 75),
    (Code::Numpad5, 76),
    (Code::Numpad6, 77),
    (Code::NumpadAdd, 78),
    (Code::Numpad1, 79),
    (Code::Numpad2, 80),
    (Code::Numpad3, 81),
    (Code::Numpad0, 82),
    (Code::NumpadDecimal, 83),
    (Code::IntlBackslash, 86),
    (Code::F11, 87),
    (Code::F12, 88),
    (Code::IntlRo, 89),
    (Code::NumpadEnter, 96),
    (Code::ControlRight, 97),
    (Code::NumpadDivide, 98),
    (Code::PrintScreen, 99),
    (Code::AltRight, 100),
    (Code::Home, 102),
    (Code::ArrowUp, 103),
    (Code::PageUp, 104),
    (Code::ArrowLeft, 105),
    (Code::ArrowRight, 106),
    (Code::End, 107),
    (Code::ArrowDown, 108),
    (Code::PageDown, 109),
    (Code::Insert, 110),
    (Code::Delete, 111),
    (Code::NumpadEqual, 117),
    (Code::Pause, 119),
    (Code::SuperLeft, 125),
    (Code::SuperRight, 126),
    (Code::ContextMenu, 127),
];

/// The Linux evdev code of a physical key, for QEMU extended key events:
/// the remote layout then decides what the key types, so non-US layouts
/// work without keysym guessing.
pub fn evdev(physical: Physical) -> Option<u16> {
    match physical {
        Physical::Code(code) => EVDEV.iter().find(|(c, _)| *c == code).map(|(_, e)| *e),
        // Unknown to the toolkit: X keycodes are evdev + 8.
        Physical::Unidentified(NativeCode::Xkb(keycode)) => {
            keycode.checked_sub(8).and_then(|k| u16::try_from(k).ok())
        }
        Physical::Unidentified(_) => None,
    }
}

#[cfg(test)]
mod evdev_tests {
    use super::*;

    #[test]
    fn physical_keys_map_to_evdev_codes() {
        assert_eq!(evdev(Physical::Code(Code::KeyA)), Some(30));
        assert_eq!(evdev(Physical::Code(Code::ArrowUp)), Some(103));
        assert_eq!(evdev(Physical::Code(Code::AltRight)), Some(100));
        assert_eq!(evdev(Physical::Unidentified(NativeCode::Xkb(38))), Some(30));
        assert_eq!(
            evdev(Physical::Unidentified(NativeCode::Unidentified)),
            None
        );
    }
}
