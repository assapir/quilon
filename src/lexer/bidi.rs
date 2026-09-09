//! Trojan Source guard (the CVE-2021-42574 class of attack): `crate::lexer::token` allows
//! a bidi control (or scopeless mark) only inside a string literal or a `~` comment, and
//! there, every opener must be closed before that token ends. The character classification
//! and the balance check both live in `quilon_rt::bidi`, shared with the `Text` header's
//! no-bidi-controls flag (`quilon_rt::mem::text_header`).

pub use quilon_rt::bidi::{ScopeStack, is_bidi_control, name};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_embedding_closes() {
        let mut stack = ScopeStack::new();
        for ch in ['\u{202B}', 'x', '\u{202C}'] {
            assert_eq!(stack.feed(ch), Ok(()));
        }
        assert_eq!(stack.unclosed(), None);
    }

    #[test]
    fn balanced_isolate_closes() {
        let mut stack = ScopeStack::new();
        for ch in ['\u{2067}', 'x', '\u{2069}'] {
            assert_eq!(stack.feed(ch), Ok(()));
        }
        assert_eq!(stack.unclosed(), None);
    }

    #[test]
    fn embedding_nested_inside_isolate_closes_in_order() {
        // RLI ... LRE ... PDF ... PDI — PDF closes the embedding (innermost), PDI closes
        // the isolate; interleaving the two families is exactly what UAX #9 nesting allows.
        let mut stack = ScopeStack::new();
        for ch in ['\u{2067}', '\u{202A}', '\u{202C}', '\u{2069}'] {
            assert_eq!(stack.feed(ch), Ok(()));
        }
        assert_eq!(stack.unclosed(), None);
    }

    #[test]
    fn unclosed_opener_is_reported() {
        let mut stack = ScopeStack::new();
        assert_eq!(stack.feed('\u{202E}'), Ok(()));
        assert_eq!(stack.unclosed(), Some('\u{202E}'));
    }

    #[test]
    fn a_closer_with_nothing_open_is_stray() {
        let mut stack = ScopeStack::new();
        assert_eq!(stack.feed('\u{202C}'), Err('\u{202C}'));
        let mut stack = ScopeStack::new();
        assert_eq!(stack.feed('\u{2069}'), Err('\u{2069}'));
    }

    #[test]
    fn pdf_cannot_close_an_isolate_and_pdi_cannot_close_an_embedding() {
        let mut stack = ScopeStack::new();
        stack.feed('\u{2067}').unwrap(); // RLI
        assert_eq!(stack.feed('\u{202C}'), Err('\u{202C}')); // PDF over an isolate: stray

        let mut stack = ScopeStack::new();
        stack.feed('\u{202B}').unwrap(); // RLE
        assert_eq!(stack.feed('\u{2069}'), Err('\u{2069}')); // PDI over an embedding: stray
    }

    #[test]
    fn scopeless_marks_never_affect_the_stack() {
        let mut stack = ScopeStack::new();
        for ch in ['\u{200E}', '\u{200F}', '\u{061C}'] {
            assert_eq!(stack.feed(ch), Ok(()));
        }
        assert_eq!(stack.unclosed(), None);
    }

    #[test]
    fn plain_characters_are_not_bidi_controls() {
        assert!(!is_bidi_control('a'));
        assert!(!is_bidi_control('ש'));
        assert!(is_bidi_control('\u{202E}'));
    }
}
