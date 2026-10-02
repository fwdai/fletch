//! The fallback handoff: the transcript itself, cut to its most recent part.

use std::borrow::Cow;

/// Hard cap, in bytes, on the fallback context. It reaches the agent inside a
/// single argv element (e.g. `--append-system-prompt <text>`), and the OS
/// rejects oversized arguments with E2BIG ("Argument list too long"): Linux
/// caps any one argument at 128 KiB (`MAX_ARG_STRLEN`), macOS caps argv + env
/// together at 1 MiB (`ARG_MAX`). 64 KiB leaves room for the rest of the brief.
/// A summary is capped well below this (`run::MAX_REPLY_BYTES`).
const MAX_CONTEXT_BYTES: usize = 64 * 1024;

/// First line of a fallback context, so the agent knows what it is reading.
pub(super) const FALLBACK_NOTICE: &str =
    "No summary could be made of the earlier conversation, so its most recent part follows as it was.";

/// First line of a transcript that [`keep_tail`] had to cut.
const OMITTED_NOTE: &str =
    "[Earlier conversation omitted to fit the size limit; the most recent part follows.]\n";

/// The context a session gets when its transcript couldn't be summarized:
/// [`FALLBACK_NOTICE`], then as much of the transcript's tail as fits in
/// [`MAX_CONTEXT_BYTES`].
pub(super) fn fallback(transcript: &str) -> String {
    let head = format!("{FALLBACK_NOTICE}\n\n");
    let tail = keep_tail(transcript, MAX_CONTEXT_BYTES - head.len());
    format!("{head}{tail}")
}

/// Fit `text` into `max` bytes (`max` must exceed [`OMITTED_NOTE`]) by keeping
/// its most recent tail — the end of the conversation is what the new session
/// continues from — behind [`OMITTED_NOTE`]. The cut lands on a char boundary
/// and, when one is near, at the start of a line.
fn keep_tail(text: &str, max: usize) -> Cow<'_, str> {
    if text.len() <= max {
        return Cow::Borrowed(text);
    }
    // How far past the byte cut to look for a line start before settling for
    // a mid-line cut, rather than give up most of the budget to reach one.
    const LINE_SEARCH: usize = 1024;

    let mut start = text.len() - (max - OMITTED_NOTE.len());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    if !text[..start].ends_with('\n') {
        let window = &text.as_bytes()[start..text.len().min(start + LINE_SEARCH)];
        if let Some(nl) = window.iter().position(|&b| b == b'\n') {
            start += nl + 1;
        }
    }
    Cow::Owned(format!("{OMITTED_NOTE}{}", &text[start..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_carries_a_short_transcript_whole_behind_the_notice() {
        let carried = fallback("User: hi\n\nAssistant: hey");
        assert_eq!(
            carried,
            format!("{FALLBACK_NOTICE}\n\nUser: hi\n\nAssistant: hey")
        );
    }

    #[test]
    fn fallback_caps_an_oversized_transcript_keeping_its_tail() {
        // ~3 MB, the size of the real forks that blew past ARG_MAX.
        let transcript: String = (0..100_000)
            .map(|i| format!("User: message {i:06}\n\n"))
            .collect();
        let carried = fallback(&transcript);
        assert!(carried.len() <= MAX_CONTEXT_BYTES);
        assert!(carried.starts_with(FALLBACK_NOTICE));
        assert!(carried.contains(OMITTED_NOTE));
        assert!(!carried.contains("message 000000"));
        assert!(carried.ends_with("User: message 099999\n\n"));
        // The cut snapped to a line start, so the first kept line is whole.
        let after_note = carried.split(OMITTED_NOTE).nth(1).unwrap();
        assert!(after_note.trim_start().starts_with("User: message "));
    }

    #[test]
    fn keep_tail_passes_text_within_the_cap_through() {
        assert_eq!(keep_tail("short", 100), "short");
        let exact = "x".repeat(100);
        assert_eq!(keep_tail(&exact, 100), exact);
    }

    #[test]
    fn keep_tail_prefers_a_line_start() {
        let text = format!("{}\nkept line\n", "a".repeat(200));
        let max = OMITTED_NOTE.len() + 14;
        // The byte cut lands mid-way through the "a" run; the next line start
        // is close, so the cut moves there.
        assert_eq!(keep_tail(&text, max), format!("{OMITTED_NOTE}kept line\n"));
    }

    #[test]
    fn keep_tail_cuts_mid_line_when_no_line_start_is_near() {
        let text = "a".repeat(10_000);
        let max = OMITTED_NOTE.len() + 100;
        let kept = keep_tail(&text, max);
        assert_eq!(kept, format!("{OMITTED_NOTE}{}", "a".repeat(100)));
    }

    #[test]
    fn keep_tail_never_splits_a_multibyte_char() {
        // 4-byte chars with every leading pad, so the raw byte cut falls at each
        // offset within a char. Slicing off a boundary would panic.
        for pad in 0..4 {
            let text = format!("{}{}", "x".repeat(pad), "😀".repeat(1_000));
            for budget in 1..12 {
                let max = OMITTED_NOTE.len() + budget;
                let kept = keep_tail(&text, max);
                assert!(kept.len() <= max, "pad {pad}, budget {budget}");
                let tail = kept.strip_prefix(OMITTED_NOTE).unwrap();
                assert_eq!(tail, "😀".repeat(budget / 4), "pad {pad}, budget {budget}");
            }
        }
    }
}
