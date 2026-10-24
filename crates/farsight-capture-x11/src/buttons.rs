//! RFB pointer button masks → X button presses and releases.
//!
//! RFB mask bit *n* maps to X button *n + 1*: bits 0–2 are left, middle and
//! right (buttons 1–3), bits 3–6 are the wheel (buttons 4–7) and bit 7 is
//! "back" (button 8).

/// A change of one X pointer button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButtonChange {
    Press(u8),
    Release(u8),
}

/// The button changes that turn mask `old` into mask `new`, releases first.
pub(crate) fn button_changes(old: u8, new: u8) -> Vec<ButtonChange> {
    let changed = old ^ new;
    let bits = (0..8u8).filter(|bit| changed & (1 << bit) != 0);
    let (pressed, released): (Vec<u8>, Vec<u8>) = bits.partition(|bit| new & (1 << bit) != 0);
    released
        .into_iter()
        .map(|bit| ButtonChange::Release(bit + 1))
        .chain(pressed.into_iter().map(|bit| ButtonChange::Press(bit + 1)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_is_empty() {
        assert_eq!(button_changes(0b101, 0b101), Vec::new());
    }

    #[test]
    fn left_middle_right_map_to_1_2_3() {
        assert_eq!(
            button_changes(0, 0b111),
            [
                ButtonChange::Press(1),
                ButtonChange::Press(2),
                ButtonChange::Press(3)
            ]
        );
    }

    #[test]
    fn wheel_and_back() {
        assert_eq!(button_changes(0, 1 << 3), [ButtonChange::Press(4)]);
        assert_eq!(button_changes(1 << 3, 0), [ButtonChange::Release(4)]);
        assert_eq!(button_changes(0, 1 << 6), [ButtonChange::Press(7)]);
        assert_eq!(button_changes(0, 1 << 7), [ButtonChange::Press(8)]);
    }

    #[test]
    fn releases_come_before_presses() {
        assert_eq!(
            button_changes(0b001, 0b100),
            [ButtonChange::Release(1), ButtonChange::Press(3)]
        );
    }
}
