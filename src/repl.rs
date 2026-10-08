//! Line-oriented input and control-command parsing.
//!
//! Input is read on a dedicated OS thread rather than via async stdin. Two
//! reasons: (1) `tokio::io::stdin` is backed by a blocking thread pool and is
//! awkward to cancel; (2) when kiro-tty eventually becomes a login shell on a
//! serial line, a plain blocking reader is the most portable, predictable
//! choice. The thread forwards each cooked line over an mpsc channel that the
//! async main loop selects on.

use std::io::BufRead;

use tokio::sync::mpsc;

/// A line of user input, already stripped of its trailing newline.
pub struct InputLine(pub String);

/// Spawn the stdin reader thread. The returned receiver yields one item per
/// line; it closes when stdin reaches EOF (e.g. the remote terminal hangs up).
pub fn spawn_stdin_reader() -> mpsc::Receiver<InputLine> {
    // A small buffer is plenty: input is human-paced and strictly turn-based.
    let (tx, rx) = mpsc::channel::<InputLine>(8);

    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut handle = stdin.lock();
        let mut buf = String::new();
        loop {
            buf.clear();
            match handle.read_line(&mut buf) {
                // EOF: the input stream closed. Drop the sender to signal the loop.
                Ok(0) => break,
                Ok(_) => {
                    // Trim only the trailing CR/LF, then apply any erase
                    // characters the line discipline let through.
                    let line = clean_line(buf.trim_end_matches(['\r', '\n']));
                    // `blocking_send` is correct here: we're on a plain OS thread,
                    // not inside the tokio runtime.
                    if tx.blocking_send(InputLine(line)).is_err() {
                        break; // main loop gone
                    }
                }
                Err(_) => break,
            }
        }
    });

    rx
}

/// Apply backspace (0x08) and delete (0x7F) as "erase previous character", and
/// drop any other C0 control bytes (tab becomes a space).
///
/// Normally the pty line discipline does erasing for us, but it only honours
/// ONE erase character (`stty erase`). Vintage terminals disagree on which key
/// that is: a DECwriter's DELETE sends 0x7F, NovaTerm on a C64 sends 0x08. When
/// the "other" one arrives, the kernel passes it through as data, and without
/// this the prompt Kiro sees would contain literal backspace bytes. Doing it
/// here makes the prompt correct regardless of which key the terminal uses.
pub fn clean_line(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '\u{08}' | '\u{7f}' => {
                out.pop();
            }
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A parsed line of input: either a wrapper control command or a prompt to send
/// to Kiro.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the wrapper help text.
    Help,
    /// Start a fresh session in the same working directory.
    New,
    /// Cancel the active turn (only meaningful while a turn is running).
    Cancel,
    /// Quit kiro-tty.
    Quit,
    /// Send this text to Kiro as a prompt.
    Prompt(String),
    /// An unrecognized `/command`.
    Unknown(String),
    /// Blank input; do nothing.
    Empty,
}

/// Parse a raw input line into a [`Command`].
///
/// Control commands are matched case-insensitively (a nod to uppercase-only
/// vintage terminals) and only when the line *starts* with `/`. Anything else
/// is treated as a prompt.
pub fn parse(line: &str) -> Command {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Command::Empty;
    }
    if let Some(rest) = trimmed.strip_prefix('/') {
        // First whitespace-delimited token is the command name.
        let name = rest.split_whitespace().next().unwrap_or("");
        return match name.to_ascii_lowercase().as_str() {
            "help" | "h" | "?" => Command::Help,
            "new" => Command::New,
            "cancel" => Command::Cancel,
            "quit" | "q" | "exit" | "bye" => Command::Quit,
            other => Command::Unknown(other.to_string()),
        };
    }
    Command::Prompt(trimmed.to_string())
}

/// The help text shown by `/help`. Kept short and ASCII for narrow terminals.
pub const HELP_TEXT: &str = "\
KIRO TTY COMMANDS:
  /help    show this help
  /new     start a fresh conversation
  /cancel  stop the current response
  /quit    exit
Type anything else to talk to Kiro.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_control_commands_case_insensitively() {
        assert_eq!(parse("/help"), Command::Help);
        assert_eq!(parse("/QUIT"), Command::Quit);
        assert_eq!(parse("  /New  "), Command::New);
        assert_eq!(parse("/cancel"), Command::Cancel);
    }

    #[test]
    fn aliases_resolve() {
        assert_eq!(parse("/q"), Command::Quit);
        assert_eq!(parse("/?"), Command::Help);
    }

    #[test]
    fn unknown_command_is_reported() {
        assert_eq!(parse("/frobnicate"), Command::Unknown("frobnicate".into()));
    }

    #[test]
    fn plain_text_is_a_prompt() {
        assert_eq!(
            parse("what files are here?"),
            Command::Prompt("what files are here?".into())
        );
    }

    #[test]
    fn backspace_and_delete_both_erase() {
        assert_eq!(clean_line("helo\u{08}lo"), "hello");
        assert_eq!(clean_line("helo\u{7f}lo"), "hello");
        // Mixed, and more erases than characters, never underflows.
        assert_eq!(clean_line("ab\u{08}\u{7f}\u{08}cd"), "cd");
    }

    #[test]
    fn other_control_bytes_are_dropped() {
        assert_eq!(clean_line("a\u{01}b\u{1b}c\td"), "abc d");
    }

    #[test]
    fn blank_is_empty() {
        assert_eq!(parse("   "), Command::Empty);
    }

    #[test]
    fn slash_in_prompt_body_is_still_a_command_only_at_start() {
        // A leading slash means command; a slash later does not.
        assert_eq!(
            parse("run cargo test/foo"),
            Command::Prompt("run cargo test/foo".into())
        );
    }
}
