//! Cleaning of peer-supplied names before the viewer shows them.
//!
//! mDNS instance names and host names come from anyone on the network. A
//! hostile name could use bidirectional overrides to disguise itself (for
//! example to look like a trusted machine) or control characters to corrupt a
//! terminal or log line. [`sanitize_name`] removes all of those.

/// The longest name, in characters, that [`sanitize_name`] returns.
pub const MAX_NAME_CHARS: usize = 64;

/// Returns `raw` with control, bidirectional and other invisible formatting
/// characters removed, runs of whitespace collapsed to one space, the ends
/// trimmed, and the result capped at [`MAX_NAME_CHARS`] characters.
///
/// The result may be empty; callers should then show something neutral
/// (for example the address).
#[must_use]
pub fn sanitize_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_NAME_CHARS * 4));
    let mut chars = 0;
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if is_hidden(c) {
            continue;
        }
        if pending_space && !out.is_empty() {
            if chars + 1 >= MAX_NAME_CHARS {
                break;
            }
            out.push(' ');
            chars += 1;
        }
        pending_space = false;
        out.push(c);
        chars += 1;
        if chars >= MAX_NAME_CHARS {
            break;
        }
    }
    out
}

/// Whether `c` is a control, bidirectional-control or invisible format
/// character (Unicode general categories Cc and Cf, plus the line and
/// paragraph separators and other invisible characters that can disguise text).
#[must_use]
pub fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'                  // soft hyphen
            | '\u{034F}'                // combining grapheme joiner
            | '\u{061C}'                // Arabic letter mark
            | '\u{115F}'..='\u{1160}'   // Hangul fillers
            | '\u{17B4}'..='\u{17B5}'   // Khmer inherent vowels
            | '\u{180B}'..='\u{180F}'   // Mongolian free variation selectors, vowel separator
            | '\u{200B}'..='\u{200F}'   // zero-width space/joiners, LRM, RLM
            | '\u{2028}'..='\u{202E}'   // separators, LRE/RLE/PDF/LRO/RLO
            | '\u{2060}'..='\u{206F}'   // word joiner, invisible operators, LRI/RLI/FSI/PDI, deprecated
            | '\u{3164}'                // Hangul filler
            | '\u{FE00}'..='\u{FE0F}'   // variation selectors
            | '\u{FEFF}'                // BOM / zero-width no-break space
            | '\u{FFA0}'                // halfwidth Hangul filler
            | '\u{FFF0}'..='\u{FFFB}'   // specials, interlinear annotation
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}' // Egyptian format controls
            | '\u{1BCA0}'..='\u{1BCA3}' // shorthand format controls
            | '\u{1D173}'..='\u{1D17A}' // musical format controls
            | '\u{E0000}'..='\u{E0FFF}' // tags, variation selectors supplement
            | '\u{F0000}'..='\u{10FFFF}' // private use planes 15-16
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn keeps_ordinary_names() {
        assert_eq!(sanitize_name("Living room PC"), "Living room PC");
        assert_eq!(sanitize_name("Büro-Mac (2)"), "Büro-Mac (2)");
        assert_eq!(sanitize_name("日本語のマシン"), "日本語のマシン");
    }

    #[test]
    fn strips_bidi_overrides() {
        // "evil\u{202E}gpj.exe" would display as "evilexe.jpg".
        assert_eq!(sanitize_name("evil\u{202E}gpj.exe"), "evilgpj.exe");
        assert_eq!(
            sanitize_name("\u{2066}a\u{2067}b\u{2068}c\u{2069}\u{200E}\u{200F}"),
            "abc"
        );
    }

    #[test]
    fn strips_controls_and_collapses_whitespace() {
        assert_eq!(sanitize_name("  a\tb\n\n c\u{7}\u{1b}[31m "), "a b c[31m");
        assert_eq!(sanitize_name("\u{0}\u{1}"), "");
        assert_eq!(sanitize_name("a\u{200B}b"), "ab");
    }

    #[test]
    fn caps_length_in_characters() {
        let long = "é".repeat(200);
        assert_eq!(sanitize_name(&long).chars().count(), MAX_NAME_CHARS);
        let spaced = format!("{} b", "a".repeat(MAX_NAME_CHARS - 1));
        let out = sanitize_name(&spaced);
        assert_eq!(out.chars().count(), MAX_NAME_CHARS - 1);
        assert!(!out.ends_with(' '));
    }

    proptest! {
        #[test]
        fn never_panics_and_output_is_clean(raw in any::<String>()) {
            let out = sanitize_name(&raw);
            prop_assert!(out.chars().count() <= MAX_NAME_CHARS);
            prop_assert!(!out.chars().any(is_hidden));
            prop_assert_eq!(out.trim(), out.as_str());
            prop_assert!(!out.contains("  "));
        }

        #[test]
        fn no_bidi_survives(prefix in "\\PC{0,20}", suffix in "\\PC{0,20}",
                            bidi in prop::sample::select(vec![
                                '\u{200E}', '\u{200F}', '\u{202A}', '\u{202B}', '\u{202C}',
                                '\u{202D}', '\u{202E}', '\u{2066}', '\u{2067}', '\u{2068}',
                                '\u{2069}', '\u{061C}'])) {
            let out = sanitize_name(&format!("{prefix}{bidi}{suffix}"));
            prop_assert!(!out.contains(bidi));
        }

        #[test]
        fn is_idempotent(raw in any::<String>()) {
            let once = sanitize_name(&raw);
            prop_assert_eq!(sanitize_name(&once), once.clone());
        }
    }
}
