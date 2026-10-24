//! Maps iced keyboard events to X11 keysyms.

use cosmic::iced::keyboard::key::Named;
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
