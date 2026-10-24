//! Keysym → keycode lookup over the core keyboard mapping, and the modifier
//! presses or releases a keysym needs.
//!
//! In the core mapping that XKB servers present, columns 0 and 1 are group 1
//! levels 1 and 2 (plain and Shift), columns 2 and 3 are group 2, and columns
//! 4 and 5 are group 1 levels 3 and 4 (`AltGr` and `AltGr`+Shift).

/// `NoSymbol`.
const NO_SYMBOL: u32 = 0;
/// `Shift_L`.
pub(crate) const XK_SHIFT_L: u32 = 0xFFE1;
/// `ISO_Level3_Shift` (`AltGr`).
pub(crate) const XK_ISO_LEVEL3_SHIFT: u32 = 0xFE03;

/// A shift level within group 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Base,
    Shift,
    AltGr,
    AltGrShift,
}

impl Level {
    const ALL: [Self; 4] = [Self::Base, Self::Shift, Self::AltGr, Self::AltGrShift];

    fn column(self) -> usize {
        match self {
            Self::Base => 0,
            Self::Shift => 1,
            Self::AltGr => 4,
            Self::AltGrShift => 5,
        }
    }

    fn from_mods(shift: bool, level3: bool) -> Self {
        match (shift, level3) {
            (false, false) => Self::Base,
            (true, false) => Self::Shift,
            (false, true) => Self::AltGr,
            (true, true) => Self::AltGrShift,
        }
    }

    pub(crate) fn shift(self) -> bool {
        matches!(self, Self::Shift | Self::AltGrShift)
    }

    pub(crate) fn level3(self) -> bool {
        matches!(self, Self::AltGr | Self::AltGrShift)
    }
}

/// A modifier this backend may press or release around a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Modifier {
    Shift,
    Level3,
}

/// Pressing or temporarily releasing a modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModChange {
    Press(Modifier),
    Release(Modifier),
}

/// The core keyboard mapping.
#[derive(Debug, Clone, Default)]
pub(crate) struct Keymap {
    min_keycode: u8,
    per_keycode: usize,
    syms: Vec<u32>,
}

impl Keymap {
    pub(crate) fn new(min_keycode: u8, per_keycode: u8, syms: Vec<u32>) -> Self {
        Self {
            min_keycode,
            per_keycode: usize::from(per_keycode),
            syms,
        }
    }

    fn keycodes(&self) -> impl Iterator<Item = u8> + '_ {
        let count = self.syms.len().checked_div(self.per_keycode).unwrap_or(0);
        (0..count).map_while(|i| self.min_keycode.checked_add(u8::try_from(i).ok()?))
    }

    fn raw(&self, keycode: u8, column: usize) -> u32 {
        if column >= self.per_keycode {
            return NO_SYMBOL;
        }
        let Some(row) = keycode.checked_sub(self.min_keycode) else {
            return NO_SYMBOL;
        };
        self.syms
            .get(usize::from(row) * self.per_keycode + column)
            .copied()
            .unwrap_or(NO_SYMBOL)
    }

    /// The keysym `keycode` produces at `level`, applying the core rule that a
    /// lone lowercase letter in column 0 implies its uppercase in column 1.
    pub(crate) fn symbol(&self, keycode: u8, level: Level) -> u32 {
        let sym = self.raw(keycode, level.column());
        if sym != NO_SYMBOL {
            return sym;
        }
        let base_column = if level.level3() { 4 } else { 0 };
        let base = self.raw(keycode, base_column);
        if level.shift() {
            upper_of(base).unwrap_or(base)
        } else {
            NO_SYMBOL
        }
    }

    /// The keycode and level that produce `keysym`, preferring lower levels.
    pub(crate) fn lookup(&self, keysym: u32) -> Option<(u8, Level)> {
        if keysym == NO_SYMBOL {
            return None;
        }
        Level::ALL.into_iter().find_map(|level| {
            self.keycodes()
                .find(|&keycode| self.symbol(keycode, level) == keysym)
                .map(|keycode| (keycode, level))
        })
    }

    /// A keycode with no symbols at all, searched from the top, for
    /// temporarily mapping a keysym that has no key.
    pub(crate) fn spare_keycode(&self) -> Option<u8> {
        let mut codes: Vec<u8> = self.keycodes().collect();
        codes.reverse();
        codes
            .into_iter()
            .find(|&keycode| (0..self.per_keycode).all(|c| self.raw(keycode, c) == NO_SYMBOL))
    }

    /// Replaces `keycode`'s symbols, mirroring a `ChangeKeyboardMapping`.
    pub(crate) fn set(&mut self, keycode: u8, syms: &[u32]) {
        let Some(row) = keycode.checked_sub(self.min_keycode) else {
            return;
        };
        let start = usize::from(row) * self.per_keycode;
        if let Some(slots) = self.syms.get_mut(start..start + self.per_keycode) {
            for (slot, sym) in slots
                .iter_mut()
                .zip(syms.iter().chain(std::iter::repeat(&NO_SYMBOL)))
            {
                *slot = *sym;
            }
        }
    }

    /// Keysyms per keycode, for `ChangeKeyboardMapping`.
    pub(crate) fn per_keycode(&self) -> u8 {
        u8::try_from(self.per_keycode).unwrap_or(u8::MAX)
    }

    /// Whether `keycode` produces something other than `keysym` at the level
    /// reached when the held modifiers are added to `level`.
    fn differs_at(&self, keycode: u8, keysym: u32, level: Level) -> bool {
        let sym = self.symbol(keycode, level);
        sym != NO_SYMBOL && sym != keysym
    }
}

/// The current modifier state and the key about to be pressed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct KeyContext {
    pub keycode: u8,
    pub keysym: u32,
    pub level: Level,
    pub shift_held: bool,
    pub level3_held: bool,
}

/// The modifier changes needed to type `ctx.keysym` on `ctx.keycode`.
///
/// A missing modifier is pressed only when it is not already held. A held
/// modifier is released only when it would change the symbol the key
/// produces, so Shift+arrow and similar combinations keep working.
pub(crate) fn plan_modifiers(map: &Keymap, ctx: KeyContext) -> Vec<ModChange> {
    if is_modifier_keysym(ctx.keysym) {
        return Vec::new();
    }
    let mut changes = Vec::new();
    let held_level = Level::from_mods(ctx.shift_held, ctx.level3_held);
    if ctx.level.shift() && !ctx.shift_held {
        changes.push(ModChange::Press(Modifier::Shift));
    } else if !ctx.level.shift()
        && ctx.shift_held
        && map.differs_at(
            ctx.keycode,
            ctx.keysym,
            Level::from_mods(true, ctx.level.level3()),
        )
    {
        changes.push(ModChange::Release(Modifier::Shift));
    }
    if ctx.level.level3() && !ctx.level3_held {
        changes.push(ModChange::Press(Modifier::Level3));
    } else if !ctx.level.level3()
        && ctx.level3_held
        && map.differs_at(
            ctx.keycode,
            ctx.keysym,
            Level::from_mods(held_level.shift(), true),
        )
    {
        changes.push(ModChange::Release(Modifier::Level3));
    }
    changes
}

/// Shift, Control, Alt, Meta, Super, Hyper, Caps/Shift Lock and the ISO
/// level/group shifts: keys the client presses explicitly.
pub(crate) fn is_modifier_keysym(keysym: u32) -> bool {
    (0xFFE1..=0xFFEE).contains(&keysym) || (0xFE01..=0xFE13).contains(&keysym) || keysym == 0xFF7E
}

/// The uppercase keysym of a Latin-1 lowercase letter.
fn upper_of(keysym: u32) -> Option<u32> {
    match keysym {
        0x61..=0x7A => Some(keysym - 0x20),
        0xE0..=0xFE if keysym != 0xF7 => Some(keysym - 0x20),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A_LOWER: u32 = 0x61;
    const A_UPPER: u32 = 0x41;
    const ONE: u32 = 0x31;
    const EXCLAM: u32 = 0x21;
    const AT: u32 = 0x40;
    const LEFT: u32 = 0xFF51;

    /// Keycodes 8..=12, six columns each.
    fn map() -> Keymap {
        #[rustfmt::skip]
        let syms = vec![
            A_LOWER, 0, 0, 0, 0, 0,             // 8: a (implied A)
            ONE, EXCLAM, 0, 0, 0, 0,            // 9: 1 !
            0x71, 0x51, 0, 0, AT, 0,            // 10: q Q @ (AltGr)
            LEFT, 0, 0, 0, 0, 0,                // 11: Left
            0, 0, 0, 0, 0, 0,                   // 12: spare
        ];
        Keymap::new(8, 6, syms)
    }

    fn ctx(keysym: u32, shift_held: bool, level3_held: bool) -> KeyContext {
        let (keycode, level) = map().lookup(keysym).unwrap();
        KeyContext {
            keycode,
            keysym,
            level,
            shift_held,
            level3_held,
        }
    }

    #[test]
    fn lookup_finds_levels() {
        let m = map();
        assert_eq!(m.lookup(A_LOWER), Some((8, Level::Base)));
        assert_eq!(m.lookup(A_UPPER), Some((8, Level::Shift)));
        assert_eq!(m.lookup(EXCLAM), Some((9, Level::Shift)));
        assert_eq!(m.lookup(AT), Some((10, Level::AltGr)));
        assert_eq!(m.lookup(0x263A), None);
        assert_eq!(m.lookup(0), None);
        assert_eq!(m.spare_keycode(), Some(12));
    }

    #[test]
    fn shift_is_pressed_only_when_not_held() {
        let m = map();
        assert_eq!(
            plan_modifiers(&m, ctx(EXCLAM, false, false)),
            [ModChange::Press(Modifier::Shift)]
        );
        assert_eq!(plan_modifiers(&m, ctx(EXCLAM, true, false)), Vec::new());
    }

    #[test]
    fn held_shift_is_released_for_base_symbol() {
        let m = map();
        assert_eq!(
            plan_modifiers(&m, ctx(ONE, true, false)),
            [ModChange::Release(Modifier::Shift)]
        );
        // Implied uppercase: Shift+a would give A.
        assert_eq!(
            plan_modifiers(&m, ctx(A_LOWER, true, false)),
            [ModChange::Release(Modifier::Shift)]
        );
    }

    #[test]
    fn held_shift_is_kept_when_it_does_not_change_the_key() {
        assert_eq!(plan_modifiers(&map(), ctx(LEFT, true, false)), Vec::new());
    }

    #[test]
    fn altgr_level_presses_level3() {
        let m = map();
        assert_eq!(
            plan_modifiers(&m, ctx(AT, false, false)),
            [ModChange::Press(Modifier::Level3)]
        );
        assert_eq!(plan_modifiers(&m, ctx(AT, false, true)), Vec::new());
        // Column 5 is empty, so Shift does not change the AltGr symbol.
        assert_eq!(
            plan_modifiers(&m, ctx(AT, true, false)),
            [ModChange::Press(Modifier::Level3)]
        );
    }

    #[test]
    fn held_altgr_is_released_for_base_symbol() {
        assert_eq!(
            plan_modifiers(&map(), ctx(0x71, false, true)),
            [ModChange::Release(Modifier::Level3)]
        );
    }

    #[test]
    fn modifier_keys_need_no_plan() {
        let m = Keymap::new(8, 2, vec![XK_SHIFT_L, 0]);
        let ctx = KeyContext {
            keycode: 8,
            keysym: XK_SHIFT_L,
            level: Level::Base,
            shift_held: true,
            level3_held: true,
        };
        assert_eq!(plan_modifiers(&m, ctx), Vec::new());
    }
}
