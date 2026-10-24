//! X11 keysym helpers (the key identifiers used by RFB `KeyEvent`).

/// Keysyms for non-character keys.
#[allow(missing_docs)]
pub mod named {
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const RETURN: u32 = 0xff0d;
    pub const PAUSE: u32 = 0xff13;
    pub const SCROLL_LOCK: u32 = 0xff14;
    pub const SYS_REQ: u32 = 0xff15;
    pub const ESCAPE: u32 = 0xff1b;
    pub const DELETE: u32 = 0xffff;
    pub const HOME: u32 = 0xff50;
    pub const LEFT: u32 = 0xff51;
    pub const UP: u32 = 0xff52;
    pub const RIGHT: u32 = 0xff53;
    pub const DOWN: u32 = 0xff54;
    pub const PAGE_UP: u32 = 0xff55;
    pub const PAGE_DOWN: u32 = 0xff56;
    pub const END: u32 = 0xff57;
    pub const PRINT: u32 = 0xff61;
    pub const INSERT: u32 = 0xff63;
    pub const MENU: u32 = 0xff67;
    pub const NUM_LOCK: u32 = 0xff7f;
    pub const F1: u32 = 0xffbe;
    pub const SHIFT_L: u32 = 0xffe1;
    pub const SHIFT_R: u32 = 0xffe2;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const CONTROL_R: u32 = 0xffe4;
    pub const CAPS_LOCK: u32 = 0xffe5;
    pub const META_L: u32 = 0xffe7;
    pub const ALT_L: u32 = 0xffe9;
    pub const ALT_R: u32 = 0xffea;
    pub const SUPER_L: u32 = 0xffeb;
    pub const SUPER_R: u32 = 0xffec;
    pub const ISO_LEVEL3_SHIFT: u32 = 0xfe03;
}

/// The keysym for function key `F<n>` (1–35).
#[must_use]
pub fn function_key(n: u8) -> Option<u32> {
    (1..=35).contains(&n).then(|| named::F1 + u32::from(n) - 1)
}

/// The keysym for a character: Latin-1 maps directly; other Unicode
/// characters use the `0x0100_0000 + codepoint` range.
#[must_use]
pub fn from_char(c: char) -> u32 {
    let code = u32::from(c);
    match code {
        0x20..=0x7e | 0xa0..=0xff => code,
        _ => 0x0100_0000 + code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters() {
        assert_eq!(from_char('a'), 0x61);
        assert_eq!(from_char('é'), 0xe9);
        assert_eq!(from_char('€'), 0x0100_20ac);
    }

    #[test]
    fn function_keys() {
        assert_eq!(function_key(1), Some(0xffbe));
        assert_eq!(function_key(12), Some(0xffc9));
        assert_eq!(function_key(0), None);
    }
}
