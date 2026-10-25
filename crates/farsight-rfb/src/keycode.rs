//! Physical key codes for the QEMU extended key event (rfbproto "QEMU
//! Extended Key Event Message"): Linux evdev codes ↔ XT scancodes as the
//! wire carries them (single-byte codes as is; `E0 xx` codes as `0x80 | xx`).

/// evdev codes 1–88 equal their XT scancodes; these are the `E0`-prefixed keys.
const EXTENDED: [(u16, u32); 18] = [
    (96, 0x9c),  // KP Enter
    (97, 0x9d),  // Right Ctrl
    (98, 0xb5),  // KP /
    (99, 0xb7),  // Print Screen / SysRq
    (100, 0xb8), // Right Alt (AltGr)
    (102, 0xc7), // Home
    (103, 0xc8), // Up
    (104, 0xc9), // Page Up
    (105, 0xcb), // Left
    (106, 0xcd), // Right
    (107, 0xcf), // End
    (108, 0xd0), // Down
    (109, 0xd1), // Page Down
    (110, 0xd2), // Insert
    (111, 0xd3), // Delete
    (125, 0xdb), // Left Super
    (126, 0xdc), // Right Super
    (127, 0xdd), // Menu
];

/// The XT code for an evdev key, if it has one.
#[must_use]
pub fn evdev_to_xt(evdev: u16) -> Option<u32> {
    match evdev {
        1..=88 => Some(u32::from(evdev)),
        _ => EXTENDED
            .iter()
            .find(|(code, _)| *code == evdev)
            .map(|(_, xt)| *xt),
    }
}

/// The evdev key for an XT code from the wire, if it is one we know.
#[must_use]
pub fn xt_to_evdev(xt: u32) -> Option<u16> {
    match xt {
        1..=88 => u16::try_from(xt).ok(),
        _ => EXTENDED
            .iter()
            .find(|(_, code)| *code == xt)
            .map(|(evdev, _)| *evdev),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_unknown_ones_are_refused() {
        for evdev in (1..=88).chain(EXTENDED.iter().map(|(e, _)| *e)) {
            assert_eq!(xt_to_evdev(evdev_to_xt(evdev).unwrap()), Some(evdev));
        }
        assert_eq!(evdev_to_xt(30), Some(0x1e), "A");
        assert_eq!(evdev_to_xt(103), Some(0xc8), "Up is E0 48");
        assert_eq!(evdev_to_xt(0), None);
        assert_eq!(xt_to_evdev(0xffff_ffff), None);
        assert_eq!(xt_to_evdev(0x80), None);
    }
}
