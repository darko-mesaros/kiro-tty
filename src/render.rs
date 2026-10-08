//! The dumb-terminal rendering layer.
//!
//! This is the reason kiro-tty exists: take whatever text the model streams and
//! make it safe for a terminal that can do little more than print characters.
//! Three transforms, in order:
//!
//!   1. **Sanitize** to 7-bit ASCII — map common Unicode punctuation to ASCII
//!      equivalents, strip ANSI/control bytes, optionally uppercase.
//!   2. **Hard-wrap** at a configurable column, word-aware, and *stateful across
//!      streamed chunks* (a word split across two chunks still wraps correctly).
//!   3. **Normalize newlines** to the terminal's convention (LF / CRLF / CR).
//!
//! The wrapper is a small state machine so it can run incrementally: `feed` is
//! called with each streamed chunk and returns bytes ready to write now, holding
//! back only a partial trailing word. `flush` emits that remainder at turn end.

use std::fmt;
use std::str::FromStr;

/// Line-ending convention for the connected terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newline {
    /// Unix `\n`.
    Lf,
    /// DOS / many serial terminals `\r\n`.
    Crlf,
    /// Classic Mac / some teletypes `\r`.
    Cr,
}

impl Newline {
    fn as_str(self) -> &'static str {
        match self {
            Newline::Lf => "\n",
            Newline::Crlf => "\r\n",
            Newline::Cr => "\r",
        }
    }
}

impl FromStr for Newline {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "lf" => Ok(Newline::Lf),
            "crlf" => Ok(Newline::Crlf),
            "cr" => Ok(Newline::Cr),
            other => Err(format!("unknown newline mode '{other}' (use lf|crlf|cr)")),
        }
    }
}

impl fmt::Display for Newline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Newline::Lf => "lf",
            Newline::Crlf => "crlf",
            Newline::Cr => "cr",
        })
    }
}

/// Streaming, word-aware renderer. Not `Clone`; holds wrap state.
pub struct Renderer {
    width: usize,
    newline: Newline,
    uppercase: bool,
    /// Current column on the logical output line.
    col: usize,
    /// Word being accumulated but not yet placed.
    word: String,
    /// Whether a separating space is owed before the next word.
    pending_space: bool,
}

impl Renderer {
    pub fn new(width: usize, newline: Newline, uppercase: bool) -> Self {
        Self {
            // A width of 0 would make wrapping meaningless; clamp to something sane.
            width: width.max(1),
            newline,
            uppercase,
            col: 0,
            word: String::new(),
            pending_space: false,
        }
    }

    pub fn newline_str(&self) -> &'static str {
        self.newline.as_str()
    }

    /// Feed a streamed chunk of agent text. Returns terminal-ready bytes to
    /// write immediately (a partial trailing word may be withheld).
    pub fn feed(&mut self, raw: &str) -> String {
        let sanitized = sanitize(raw, self.uppercase);
        let mut logical = String::new();
        for c in sanitized.chars() {
            match c {
                '\n' => {
                    self.flush_word(&mut logical);
                    logical.push('\n');
                    self.col = 0;
                    self.pending_space = false;
                }
                ' ' | '\t' => {
                    self.flush_word(&mut logical);
                    // Only owe a space if we're mid-line; avoids leading spaces.
                    if self.col > 0 {
                        self.pending_space = true;
                    }
                }
                _ => self.word.push(c),
            }
        }
        self.convert_newlines(&logical)
    }

    /// Flush any pending word and terminate the current line. Call at the end of
    /// a streamed message so nothing is left buffered.
    pub fn flush(&mut self) -> String {
        let mut logical = String::new();
        self.flush_word(&mut logical);
        if self.col > 0 {
            logical.push('\n');
            self.col = 0;
        }
        self.pending_space = false;
        self.convert_newlines(&logical)
    }

    /// Emit a self-contained UI line (banner, prompt, tool status). Sanitized,
    /// word-wrapped to the terminal width (words longer than the width are
    /// hard-split), and newline-terminated. Also forces a break if we were
    /// mid-stream.
    pub fn ui_line(&mut self, text: &str) -> String {
        let mut out = String::new();
        // If the agent stream left us mid-line, break first.
        if self.col > 0 || !self.word.is_empty() {
            out.push_str(&self.flush());
        }
        let mut logical = wrap_words(&sanitize(text, self.uppercase), self.width);
        logical.push('\n');
        out.push_str(&self.convert_newlines(&logical));
        out
    }

    /// Place the pending word onto the logical output, wrapping as needed.
    fn flush_word(&mut self, out: &mut String) {
        if self.word.is_empty() {
            return;
        }
        let word = std::mem::take(&mut self.word);
        let wlen = word.chars().count();

        if self.pending_space {
            // Wrap instead of emitting a trailing space if the word wouldn't fit.
            if self.col > 0 && self.col + 1 + wlen > self.width {
                out.push('\n');
                self.col = 0;
            } else if self.col > 0 {
                out.push(' ');
                self.col += 1;
            }
            self.pending_space = false;
        } else if self.col > 0 && self.col + wlen > self.width {
            out.push('\n');
            self.col = 0;
        }

        // Place the word, hard-splitting only if it alone exceeds the width
        // (e.g. a very long URL or path).
        for ch in word.chars() {
            if self.col >= self.width {
                out.push('\n');
                self.col = 0;
            }
            out.push(ch);
            self.col += 1;
        }
    }

    /// Convert logical `\n` breaks into the configured terminal newline.
    fn convert_newlines(&self, logical: &str) -> String {
        match self.newline {
            Newline::Lf => logical.to_string(),
            other => logical.replace('\n', other.as_str()),
        }
    }
}

/// Map arbitrary text to a 7-bit-ASCII-safe subset suitable for a dumb terminal.
///
/// - Common Unicode punctuation is transliterated to ASCII.
/// - ANSI escape sequences (ESC + CSI) are stripped.
/// - Other control bytes are dropped (tab becomes a space; newline is kept by
///   the caller's char loop, so here we normalize CR/CRLF into `\n`).
/// - Remaining non-ASCII is replaced with '?'.
pub fn sanitize(input: &str, uppercase: bool) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // Normalize line endings to a single '\n'; the wrapper owns layout.
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            // Strip ANSI escape sequences defensively.
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    // Consume until a final byte in the range @..~ (0x40..=0x7e).
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if ('\u{40}'..='\u{7e}').contains(&nc) {
                            break;
                        }
                    }
                }
                // Lone ESC or other escapes: drop the ESC itself.
            }
            // Transliterate common Unicode punctuation.
            '\u{2018}' | '\u{2019}' | '\u{201B}' => out.push('\''),
            '\u{201C}' | '\u{201D}' | '\u{201E}' => out.push('"'),
            '\u{2013}' | '\u{2014}' | '\u{2212}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            '\u{00A0}' | '\u{2007}' | '\u{202F}' => out.push(' '),
            '\u{2022}' | '\u{25CF}' | '\u{25AA}' => out.push('*'),
            '\u{2192}' => out.push_str("->"),
            '\u{2190}' => out.push_str("<-"),
            _ => {
                if c.is_ascii() {
                    // Keep printable ASCII; drop other control chars.
                    if c == ' ' || !c.is_ascii_control() {
                        out.push(c);
                    }
                } else {
                    out.push('?');
                }
            }
        }
    }

    if uppercase {
        out = out.to_ascii_uppercase();
    }
    out
}

/// Greedy word-wrap of one logical line to `width` columns, joining with `\n`.
/// Words longer than `width` are hard-split so no output line ever exceeds it.
fn wrap_words(text: &str, width: usize) -> String {
    let width = width.max(1);
    let mut out = String::new();
    let mut col = 0usize;
    for word in text.split_whitespace() {
        let mut chars: Vec<char> = word.chars().collect();
        // Break before the word if it would not fit on the current line.
        if col > 0 && col + 1 + chars.len() > width {
            out.push('\n');
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        }
        // Hard-split anything longer than a whole line.
        while chars.len() > width - col {
            let rest = chars.split_off(width - col);
            out.extend(chars);
            out.push('\n');
            chars = rest;
            col = 0;
        }
        col += chars.len();
        out.extend(chars);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_line_wraps_to_width() {
        let mut r = Renderer::new(40, Newline::Lf, false);
        let out = r.ui_line("*** DANGER MODE: all tools auto-approved ***");
        assert!(out.lines().all(|l| l.chars().count() <= 40), "{out:?}");
        assert_eq!(out, "*** DANGER MODE: all tools auto-approved\n***\n");
    }

    #[test]
    fn wrap_words_hard_splits_long_words() {
        assert_eq!(wrap_words("ab /very/long/path", 6), "ab\n/very/\nlong/p\nath");
        assert_eq!(wrap_words("short line", 80), "short line");
    }

    #[test]
    fn sanitize_transliterates_smart_punctuation() {
        let input = "\u{201C}hello\u{201D} \u{2014} world\u{2026}";
        assert_eq!(sanitize(input, false), "\"hello\" - world...");
    }

    #[test]
    fn sanitize_strips_ansi() {
        let input = "\u{1b}[31mred\u{1b}[0m text";
        assert_eq!(sanitize(input, false), "red text");
    }

    #[test]
    fn sanitize_replaces_unknown_non_ascii() {
        assert_eq!(sanitize("caf\u{e9} \u{1f600}", false), "caf? ?");
    }

    #[test]
    fn sanitize_uppercase() {
        assert_eq!(sanitize("Hello", true), "HELLO");
    }

    #[test]
    fn wrap_breaks_on_word_boundary() {
        let mut r = Renderer::new(10, Newline::Lf, false);
        let mut out = r.feed("the quick brown fox");
        out.push_str(&r.flush());
        // "the quick" = 9 chars fits; "brown" would push to 15 -> wrap.
        assert_eq!(out, "the quick\nbrown fox\n");
    }

    #[test]
    fn wrap_is_stateful_across_chunks() {
        let mut r = Renderer::new(10, Newline::Lf, false);
        let mut out = r.feed("hello wor");
        out.push_str(&r.feed("ld friend"));
        out.push_str(&r.flush());
        // "helloworld" arrives split; it must be treated as one 10-char word.
        assert_eq!(out, "hello\nworld\nfriend\n");
    }

    #[test]
    fn wrap_hard_splits_overlong_word() {
        let mut r = Renderer::new(5, Newline::Lf, false);
        let mut out = r.feed("abcdefghij");
        out.push_str(&r.flush());
        assert_eq!(out, "abcde\nfghij\n");
    }

    #[test]
    fn crlf_conversion() {
        let mut r = Renderer::new(80, Newline::Crlf, false);
        let out = r.feed("line one\nline two\n");
        assert_eq!(out, "line one\r\nline two\r\n");
    }

    #[test]
    fn ui_line_breaks_midstream() {
        let mut r = Renderer::new(80, Newline::Lf, false);
        let mut out = r.feed("partial");
        out.push_str(&r.ui_line("[TOOL] read"));
        assert_eq!(out, "partial\n[TOOL] read\n");
    }
}
