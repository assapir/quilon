//! Trojan Source guard (the CVE-2021-42574 class of attack): bidi control characters that
//! can make a token's *displayed* order diverge from its *logical* byte order, so a
//! reviewer sees something other than what the compiler reads. UAX #9 defines two families
//! of directional control: LRE/RLE/LRO/RLO/LRI/RLI/FSI each *open* a scope that a matching
//! closer must end — PDF for the first four, PDI for the isolates — while LRM/RLM/ALM are
//! scopeless marks with no closer at all. `crate::lexer::token` only allows any of these
//! inside a string literal or a `~` comment, and there, every opener must be closed before
//! that token ends; this module is the shared balance check both call.

/// How one bidi control behaves, per UAX #9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    /// LRE / RLE / LRO / RLO: opens a scope, closed by U+202C POP DIRECTIONAL FORMATTING.
    OpenEmbedding,
    /// U+202C POP DIRECTIONAL FORMATTING: closes the innermost embedding/override.
    CloseEmbedding,
    /// LRI / RLI / FSI: opens a scope, closed by U+2069 POP DIRECTIONAL ISOLATE.
    OpenIsolate,
    /// U+2069 POP DIRECTIONAL ISOLATE: closes the innermost isolate.
    CloseIsolate,
    /// LRM / RLM / ALM: no scope, never needs closing.
    Mark,
}

fn classify(ch: char) -> Option<Control> {
    match ch {
        '\u{202A}' | '\u{202B}' | '\u{202D}' | '\u{202E}' => Some(Control::OpenEmbedding),
        '\u{202C}' => Some(Control::CloseEmbedding),
        '\u{2066}' | '\u{2067}' | '\u{2068}' => Some(Control::OpenIsolate),
        '\u{2069}' => Some(Control::CloseIsolate),
        '\u{200E}' | '\u{200F}' | '\u{061C}' => Some(Control::Mark),
        _ => None,
    }
}

/// Whether `ch` is one of the bidi controls this guard recognizes at all (opener, closer,
/// or scopeless mark) — what makes a bare occurrence outside a literal/comment an error.
pub fn is_bidi_control(ch: char) -> bool {
    classify(ch).is_some()
}

/// The formal Unicode name for a recognized control, so a lex error can name it — the
/// character itself renders invisibly (that is the whole problem it causes).
pub fn name(ch: char) -> &'static str {
    match ch {
        '\u{202A}' => "U+202A LEFT-TO-RIGHT EMBEDDING",
        '\u{202B}' => "U+202B RIGHT-TO-LEFT EMBEDDING",
        '\u{202C}' => "U+202C POP DIRECTIONAL FORMATTING",
        '\u{202D}' => "U+202D LEFT-TO-RIGHT OVERRIDE",
        '\u{202E}' => "U+202E RIGHT-TO-LEFT OVERRIDE",
        '\u{2066}' => "U+2066 LEFT-TO-RIGHT ISOLATE",
        '\u{2067}' => "U+2067 RIGHT-TO-LEFT ISOLATE",
        '\u{2068}' => "U+2068 FIRST STRONG ISOLATE",
        '\u{2069}' => "U+2069 POP DIRECTIONAL ISOLATE",
        '\u{200E}' => "U+200E LEFT-TO-RIGHT MARK",
        '\u{200F}' => "U+200F RIGHT-TO-LEFT MARK",
        '\u{061C}' => "U+061C ARABIC LETTER MARK",
        _ => "an unrecognized bidi control",
    }
}

/// The open embedding/override and isolate scopes seen so far in one string literal or
/// comment, in nesting order — a stack, per UAX #9's own nesting rule (a closing PDF closes
/// the innermost embedding/override, a closing PDI the innermost isolate, regardless of how
/// the two families interleave).
#[derive(Debug, Default)]
pub struct ScopeStack(Vec<(char, Control)>);

impl ScopeStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one character. A non-control, an opener, or a mark always succeeds. A closer
    /// succeeds only when the innermost open scope is of its own kind; otherwise `ch` is a
    /// stray closer — nothing of its kind is open — and is returned as the error.
    pub fn feed(&mut self, ch: char) -> Result<(), char> {
        match classify(ch) {
            Some(Control::OpenEmbedding) => self.0.push((ch, Control::OpenEmbedding)),
            Some(Control::OpenIsolate) => self.0.push((ch, Control::OpenIsolate)),
            Some(Control::CloseEmbedding) => match self.0.last() {
                Some((_, Control::OpenEmbedding)) => {
                    self.0.pop();
                }
                _ => return Err(ch),
            },
            Some(Control::CloseIsolate) => match self.0.last() {
                Some((_, Control::OpenIsolate)) => {
                    self.0.pop();
                }
                _ => return Err(ch),
            },
            Some(Control::Mark) | None => {}
        }
        Ok(())
    }

    /// The innermost opener still unclosed once the token has ended, if any — reported as
    /// the offender, since it is the one whose scope genuinely never closed.
    pub fn unclosed(&self) -> Option<char> {
        self.0.last().map(|(ch, _)| *ch)
    }
}

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
