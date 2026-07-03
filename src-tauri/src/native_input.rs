//! Line-discipline tracking for the native (PTY/TUI) view's user input.
//!
//! The supervisor forwards raw keystroke bytes to the agent's PTY; this
//! tracker mirrors just enough terminal line editing (printable chars,
//! backspace, Ctrl-C/Ctrl-U clears, ANSI escape sequences) to reconstruct
//! the lines the user actually submits with Enter. Those submitted lines
//! feed turn-start detection and first-message task capture — nothing here
//! touches the PTY itself.

/// Per-agent input-line state. One tracker per live native-view agent;
/// `observe` consumes each outgoing byte chunk and returns any lines the
/// chunk completed.
#[derive(Default)]
pub struct NativeInputTracker {
    line: String,
}

impl NativeInputTracker {
    /// Feed one chunk of user keystroke bytes; returns the trimmed,
    /// non-empty lines submitted (Enter pressed) within this chunk.
    pub fn observe(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut submitted = Vec::new();
        let line = &mut self.line;
        let mut i = 0;

        while i < bytes.len() {
            match bytes[i] {
                b'\r' | b'\n' => {
                    let trimmed = line.trim().to_string();
                    line.clear();
                    if !trimmed.is_empty() {
                        submitted.push(trimmed);
                    }
                    i += 1;
                }
                0x7f | 0x08 => {
                    line.pop();
                    i += 1;
                }
                0x03 | 0x15 => {
                    line.clear();
                    i += 1;
                }
                0x1b => {
                    i = skip_escape_sequence(bytes, i);
                }
                b if b < 0x20 => {
                    i += 1;
                }
                _ => match std::str::from_utf8(&bytes[i..]) {
                    Ok(rest) => {
                        if let Some(ch) = rest.chars().next() {
                            line.push(ch);
                            i += ch.len_utf8();
                        } else {
                            break;
                        }
                    }
                    Err(e) => {
                        let valid = e.valid_up_to();
                        if valid > 0 {
                            if let Ok(s) = std::str::from_utf8(&bytes[i..i + valid]) {
                                if let Some(ch) = s.chars().next() {
                                    line.push(ch);
                                    i += ch.len_utf8();
                                } else {
                                    i += valid;
                                }
                            } else {
                                i += valid;
                            }
                        } else {
                            i += 1;
                        }
                    }
                },
            }
        }

        submitted
    }
}

fn skip_escape_sequence(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    if i < bytes.len() && bytes[i] == b'[' {
        i += 1;
        while i < bytes.len() {
            let b = bytes[i];
            i += 1;
            if (0x40..=0x7e).contains(&b) {
                break;
            }
        }
        return i;
    }
    if i < bytes.len() {
        i + 1
    } else {
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_submits_the_trimmed_line() {
        let mut t = NativeInputTracker::default();
        assert_eq!(t.observe(b"  hello world  \r"), vec!["hello world"]);
    }

    #[test]
    fn line_accumulates_across_chunks() {
        // Keystrokes arrive one PTY write at a time; the line must survive
        // chunk boundaries until Enter.
        let mut t = NativeInputTracker::default();
        assert!(t.observe(b"hel").is_empty());
        assert!(t.observe(b"lo").is_empty());
        assert_eq!(t.observe(b"\n"), vec!["hello"]);
    }

    #[test]
    fn empty_or_whitespace_lines_are_not_submitted() {
        let mut t = NativeInputTracker::default();
        assert!(t.observe(b"\r").is_empty());
        assert!(t.observe(b"   \n").is_empty());
    }

    #[test]
    fn backspace_removes_the_last_char() {
        let mut t = NativeInputTracker::default();
        assert_eq!(t.observe(b"abcd\x7f\x7f\r"), vec!["ab"]);
        // 0x08 (BS) behaves the same.
        assert_eq!(t.observe(b"xy\x08\r"), vec!["x"]);
    }

    #[test]
    fn ctrl_c_and_ctrl_u_clear_the_pending_line() {
        let mut t = NativeInputTracker::default();
        assert!(t.observe(b"discarded\x03").is_empty());
        assert_eq!(t.observe(b"kept\r"), vec!["kept"]);
        assert!(t.observe(b"also gone\x15").is_empty());
        assert_eq!(t.observe(b"kept too\r"), vec!["kept too"]);
    }

    #[test]
    fn csi_escape_sequences_are_skipped() {
        // Arrow keys etc. (ESC [ ... final-byte) must not leak into the line.
        let mut t = NativeInputTracker::default();
        assert_eq!(t.observe(b"a\x1b[Cb\r"), vec!["ab"]);
    }

    #[test]
    fn short_escape_sequences_are_skipped() {
        // Two-byte sequences like ESC O (application cursor keys prefix).
        let mut t = NativeInputTracker::default();
        assert_eq!(t.observe(b"a\x1bOb\r"), vec!["ab"]);
    }

    #[test]
    fn other_control_bytes_are_ignored() {
        let mut t = NativeInputTracker::default();
        assert_eq!(t.observe(b"a\x01\x02b\t\r"), vec!["ab"]);
    }

    #[test]
    fn multibyte_utf8_is_preserved() {
        let mut t = NativeInputTracker::default();
        assert_eq!(t.observe("héllo ☃\r".as_bytes()), vec!["héllo ☃"]);
    }

    #[test]
    fn invalid_utf8_bytes_are_skipped_without_panicking() {
        let mut t = NativeInputTracker::default();
        assert_eq!(t.observe(b"a\xffb\r"), vec!["ab"]);
    }

    #[test]
    fn multiple_lines_in_one_chunk() {
        let mut t = NativeInputTracker::default();
        assert_eq!(t.observe(b"one\rtwo\r"), vec!["one", "two"]);
    }
}
