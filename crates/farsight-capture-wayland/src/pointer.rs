//! Turning RFB pointer events into portal pointer calls.
//!
//! RFB sends the whole button mask with every event: bit 0 left, 1 middle, 2 right,
//! 3/4 wheel up/down, 5/6 wheel left/right, 7 back. The portal wants Linux evdev
//! button presses and releases plus discrete scroll steps, so we diff masks.

/// evdev `BTN_LEFT`.
pub const BTN_LEFT: i32 = 0x110;
/// evdev `BTN_RIGHT`.
pub const BTN_RIGHT: i32 = 0x111;
/// evdev `BTN_MIDDLE`.
pub const BTN_MIDDLE: i32 = 0x112;
/// evdev `BTN_SIDE` (the "back" thumb button).
pub const BTN_SIDE: i32 = 0x113;

/// RFB mask bits that are held buttons, with their evdev codes.
const BUTTONS: [(u8, i32); 4] = [
    (1 << 0, BTN_LEFT),
    (1 << 1, BTN_MIDDLE),
    (1 << 2, BTN_RIGHT),
    (1 << 7, BTN_SIDE),
];

/// RFB mask bits that are wheel clicks, with their axis and step direction.
const WHEEL: [(u8, ScrollAxis, i32); 4] = [
    (1 << 3, ScrollAxis::Vertical, -1),
    (1 << 4, ScrollAxis::Vertical, 1),
    (1 << 5, ScrollAxis::Horizontal, -1),
    (1 << 6, ScrollAxis::Horizontal, 1),
];

/// A scroll axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    /// Up (negative) and down (positive).
    Vertical,
    /// Left (negative) and right (positive).
    Horizontal,
}

/// One pointer call to make on the portal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerAction {
    /// An evdev button changed state.
    Button {
        /// evdev code.
        code: i32,
        /// `true` for press, `false` for release.
        pressed: bool,
    },
    /// One discrete wheel step.
    Scroll {
        /// Which axis.
        axis: ScrollAxis,
        /// -1 or +1 (up/left is negative).
        steps: i32,
    },
}

/// The calls that move the button state from `previous` to `current`.
///
/// Releases come before presses, so a client that swaps buttons in one event never
/// leaves two held at once. A wheel bit scrolls once when it goes from clear to set;
/// its release does nothing.
#[must_use]
pub fn actions(previous: u8, current: u8) -> Vec<PointerAction> {
    let released = previous & !current;
    let pressed = current & !previous;
    let mut out = Vec::new();
    out.extend(
        BUTTONS
            .iter()
            .filter(|(bit, _)| released & bit != 0)
            .map(|&(_, code)| PointerAction::Button {
                code,
                pressed: false,
            }),
    );
    out.extend(
        BUTTONS
            .iter()
            .filter(|(bit, _)| pressed & bit != 0)
            .map(|&(_, code)| PointerAction::Button {
                code,
                pressed: true,
            }),
    );
    out.extend(
        WHEEL
            .iter()
            .filter(|(bit, _, _)| pressed & bit != 0)
            .map(|&(_, axis, steps)| PointerAction::Scroll { axis, steps }),
    );
    out
}

/// Scales a framebuffer coordinate (`0..from`) into the stream's logical space (`0..to`).
///
/// With an unknown (zero) size on either side the coordinate passes through unchanged.
/// The result is clamped to `[0, to)`.
#[must_use]
pub fn scale(value: u16, from: u32, to: u32) -> f64 {
    if from == 0 || to == 0 {
        return f64::from(value);
    }
    let scaled = f64::from(value) * f64::from(to) / f64::from(from);
    scaled.clamp(
        0.0,
        f64::from(to) - f64::EPSILON.max(f64::from(to) * f64::EPSILON),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn button(code: i32, pressed: bool) -> PointerAction {
        PointerAction::Button { code, pressed }
    }

    #[test]
    fn press_and_release_left() {
        assert_eq!(actions(0, 0b1), [button(BTN_LEFT, true)]);
        assert_eq!(actions(0b1, 0b1), []);
        assert_eq!(actions(0b1, 0), [button(BTN_LEFT, false)]);
    }

    #[test]
    fn maps_middle_right_and_back() {
        assert_eq!(actions(0, 0b10), [button(BTN_MIDDLE, true)]);
        assert_eq!(actions(0, 0b100), [button(BTN_RIGHT, true)]);
        assert_eq!(actions(0, 0x80), [button(BTN_SIDE, true)]);
    }

    #[test]
    fn releases_come_before_presses() {
        assert_eq!(
            actions(0b001, 0b100),
            [button(BTN_LEFT, false), button(BTN_RIGHT, true)]
        );
    }

    #[test]
    fn wheel_scrolls_on_press_only() {
        let up = PointerAction::Scroll {
            axis: ScrollAxis::Vertical,
            steps: -1,
        };
        assert_eq!(actions(0, 1 << 3), [up]);
        assert_eq!(actions(1 << 3, 0), []);
        assert_eq!(
            actions(0, 1 << 4),
            [PointerAction::Scroll {
                axis: ScrollAxis::Vertical,
                steps: 1
            }]
        );
        assert_eq!(
            actions(0, 1 << 5),
            [PointerAction::Scroll {
                axis: ScrollAxis::Horizontal,
                steps: -1
            }]
        );
        assert_eq!(
            actions(0, 1 << 6),
            [PointerAction::Scroll {
                axis: ScrollAxis::Horizontal,
                steps: 1
            }]
        );
    }

    #[test]
    fn scales_between_sizes() {
        assert!((scale(960, 1920, 1280) - 640.0).abs() < 1e-9);
        assert!((scale(0, 1920, 1280)).abs() < 1e-9);
        assert!(scale(1919, 1920, 960) < 960.0);
        assert!((scale(10, 0, 100) - 10.0).abs() < 1e-9);
        assert!((scale(10, 100, 0) - 10.0).abs() < 1e-9);
    }

    proptest! {
        #[test]
        fn replaying_actions_reaches_the_new_state(previous in any::<u8>(), current in any::<u8>()) {
            let held = |mask: u8| BUTTONS.iter().filter(|(bit, _)| mask & bit != 0).map(|&(_, c)| c).collect::<Vec<_>>();
            let mut state = held(previous);
            for action in actions(previous, current) {
                if let PointerAction::Button { code, pressed } = action {
                    if pressed {
                        prop_assert!(!state.contains(&code));
                        state.push(code);
                    } else {
                        prop_assert!(state.contains(&code));
                        state.retain(|c| *c != code);
                    }
                }
            }
            state.sort_unstable();
            let mut expected = held(current);
            expected.sort_unstable();
            prop_assert_eq!(state, expected);
        }

        #[test]
        fn scaled_point_stays_inside(value in any::<u16>(), from in 1u32..70_000, to in 1u32..70_000) {
            let v = scale(value.min(u16::try_from(from - 1).unwrap_or(u16::MAX)), from, to);
            prop_assert!(v >= 0.0 && v < f64::from(to));
        }
    }
}
