//! Automated `claude setup-token` capture (the auto path for [`super::auth`]).
//!
//! Manually running `claude setup-token` and pasting the `sk-ant-oat…` output
//! into settings is the main "it doesn't just work" gap for docker agents. This
//! module drives that flow for the user: it spawns `claude setup-token` under a
//! PTY, surfaces the consent URL, relays the CLI's auth-code prompt to the UI,
//! writes the code the user pastes back, and captures the emitted token — no
//! copy-paste. The token is stored via the exact same persist-then-mirror path
//! the paste command uses (see `store_container_token` in `lib.rs`).
//!
//! `claude` is an Ink (React) TUI, so its output is a stream of ANSI escapes and
//! cursor moves, redrawn frame-by-frame — not clean line-oriented stdout. Two
//! consequences shape the parsing here:
//!   * We [`strip_ansi`] before scanning, and match on collapsed content
//!     (spacing between words is drawn with cursor-column escapes, not spaces).
//!   * We run the PTY at a deliberately **wide** width so the long consent URL
//!     and the long token each render on a single unbroken line, instead of
//!     wrapping mid-value where a regex/scan would tear them.
//!
//! Token discipline (invariant shared with [`super::auth`]): nothing here logs
//! the raw PTY buffer or the token. The captured stream lives only in memory and
//! dies with the session.

use std::path::Path;
use std::sync::mpsc::Sender;

use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::pty_session::{PtyExit, PtySession, PtySpawn};
use crate::sandbox::KillHandle;

/// How long `on_exit` waits for the output pipeline to drain the final bytes
/// (the token line) after the child exits, before concluding no token arrived.
/// The reader → coalescer flushes within one ~16ms frame of EOF, so this is
/// generous; it only ever elapses on a genuine no-token exit (error / cancel).
const EXIT_DRAIN_GRACE: Duration = Duration::from_secs(1);

/// PTY width. Wide enough that the ~250-char consent URL and the token both fit
/// on one line — narrow terminals wrap them mid-value (see module docs).
const PTY_COLS: u16 = 400;
const PTY_ROWS: u16 = 50;

/// Prefix every Anthropic secret shares; the setup-token's `sk-ant-oat…` is one
/// shape of it. We anchor extraction here (rather than the stricter `sk-ant-oat`)
/// so a future prefix variant is still captured — [`super::auth::normalize_token`]
/// then decides whether it's the *recognized* shape or a warn-but-store one.
const TOKEN_ANCHOR: &str = "sk-ant-";

/// Lifecycle signals the driver hands up to the UI layer.
pub enum SetupEvent {
    /// The consent URL `claude` printed. Surfaced so the user can open it if the
    /// CLI's own browser-open didn't fire (we do *not* auto-open — `claude`
    /// already tries, and a second open would spawn a duplicate tab).
    Url(String),
    /// `claude` is now blocked on its "Paste code here" stdin prompt.
    AwaitingCode,
}

/// Mutable state shared between the PTY's output and exit callbacks.
struct Parse {
    /// Everything read from the PTY so far (raw, with ANSI). Scanned on each
    /// chunk; never logged.
    raw: String,
    url_sent: bool,
    prompt_sent: bool,
    /// Delivers the final outcome exactly once. `take`-n on the first of: token
    /// captured (output path) or process exit (exit path); `None` thereafter.
    tx: Option<Sender<Result<String>>>,
}

/// A live `claude setup-token` PTY session. Held in Tauri managed state so the
/// code-submit and cancel commands can reach it; dropping it kills the PTY.
pub struct ClaudeSetup {
    pty: PtySession,
}

impl ClaudeSetup {
    /// Spawn `claude setup-token` under a wide PTY and wire parsing.
    ///
    /// `emit` receives [`SetupEvent`]s as they're detected. `tx` receives the
    /// final `Ok(token)` / `Err(reason)` exactly once. Both callbacks run on the
    /// PTY's own threads.
    pub fn start(
        claude_bin: &Path,
        cwd: &Path,
        emit: Arc<dyn Fn(SetupEvent) + Send + Sync>,
        tx: Sender<Result<String>>,
    ) -> Result<Self> {
        let parse = Arc::new(Mutex::new(Parse {
            raw: String::new(),
            url_sent: false,
            prompt_sent: false,
            tx: Some(tx),
        }));

        let on_output = {
            let parse = parse.clone();
            move |bytes: Vec<u8>| {
                let mut p = parse.lock();
                if p.tx.is_none() {
                    return; // already resolved — ignore trailing frames
                }
                p.raw.push_str(&String::from_utf8_lossy(&bytes));

                // Token wins: once it's present (and terminated, so we don't
                // capture a still-streaming prefix) we're done.
                if let Some(token) = extract_streaming_token(&p.raw) {
                    if let Some(tx) = p.tx.take() {
                        let _ = tx.send(Ok(token));
                    }
                    return;
                }
                if !p.url_sent {
                    if let Some(url) = find_consent_url(&p.raw) {
                        p.url_sent = true;
                        emit(SetupEvent::Url(url));
                    }
                }
                if !p.prompt_sent && awaiting_code(&p.raw) {
                    p.prompt_sent = true;
                    emit(SetupEvent::AwaitingCode);
                }
            }
        };

        let on_exit = {
            let parse = parse.clone();
            move |exit: PtyExit| {
                // `wait()` (this thread) races the reader → coalescer → on_output
                // pipeline: on a fast token-then-exit, the final bytes may not be
                // in `p.raw` yet. So don't conclude "no token" immediately — poll
                // for a grace window, bailing the moment either the output path
                // resolves (tx taken) or the token lands in the drained buffer.
                let deadline = Instant::now() + EXIT_DRAIN_GRACE;
                loop {
                    {
                        let mut p = parse.lock();
                        if p.tx.is_none() {
                            return; // output path already delivered the token
                        }
                        // EOF means no more bytes, so a lenient scan is safe.
                        if let Some(token) = extract_setup_token(&p.raw) {
                            if let Some(tx) = p.tx.take() {
                                let _ = tx.send(Ok(token));
                            }
                            return;
                        }
                        if Instant::now() >= deadline {
                            if let Some(tx) = p.tx.take() {
                                let _ = tx.send(Err(crate::error::Error::Other(format!(
                                    "Claude exited before returning a token ({}).",
                                    exit.message
                                ))));
                            }
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };

        let args = ["setup-token".to_string()];
        let pty = PtySession::spawn(
            PtySpawn {
                program: claude_bin,
                args: &args,
                cwd,
                env: &[],
                cols: PTY_COLS,
                rows: PTY_ROWS,
                kill_plan: KillHandle::ProcessGroup,
            },
            on_output,
            on_exit,
        )?;
        Ok(Self { pty })
    }

    /// Write the user's auth code to the CLI's stdin prompt (trailing CR submits
    /// it, as if typed at the terminal).
    pub fn submit_code(&self, code: &str) -> Result<()> {
        let mut line = code.trim().to_string();
        line.push('\r');
        self.pty.write(line.as_bytes())
    }
}

/// Strip ANSI/VT control sequences (CSI, OSC, and two-byte escapes), preserving
/// the surrounding UTF-8 bytes intact. Escape sequences are pure ASCII, so we
/// can drop them at the byte level without splitting a multibyte char.
pub fn strip_ansi(input: &str) -> String {
    let b = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b {
            i += 1;
            let Some(&kind) = b.get(i) else { break };
            match kind {
                b'[' => {
                    // CSI: params/intermediates until a final byte in @..~.
                    i += 1;
                    while i < b.len() && !(0x40..=0x7e).contains(&b[i]) {
                        i += 1;
                    }
                    if i < b.len() {
                        i += 1;
                    }
                }
                b']' => {
                    // OSC: until BEL, or ST (ESC \).
                    i += 1;
                    while i < b.len() && b[i] != 0x07 {
                        if b[i] == 0x1b && b.get(i + 1) == Some(&b'\\') {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                    if i < b.len() {
                        i += 1; // consume the BEL / the `\` of ST
                    }
                }
                // Two-byte escape (e.g. ESC 7 save-cursor, ESC = keypad mode).
                _ => i += 1,
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_token_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Locate the setup token in (possibly ANSI-laden) `output`. Anchored on
/// `sk-ant-`; returns the full token run, or `None` if there's no run past the
/// bare anchor. When `require_terminated`, a run reaching the very end of the
/// buffer is treated as still-streaming and rejected (see [`extract_streaming_token`]).
fn find_token(output: &str, require_terminated: bool) -> Option<String> {
    let s = strip_ansi(output);
    let bytes = s.as_bytes();
    let anchor = TOKEN_ANCHOR.as_bytes();
    let mut from = 0;
    while let Some(rel) = find_subslice(&bytes[from..], anchor) {
        let start = from + rel;
        let mut end = start + anchor.len();
        while end < bytes.len() && is_token_byte(bytes[end]) {
            end += 1;
        }
        if end > start + anchor.len() {
            // A run that ends exactly at the buffer edge may still be arriving.
            if require_terminated && end == bytes.len() {
                return None;
            }
            return Some(s[start..end].to_string());
        }
        from = start + anchor.len();
    }
    None
}

/// Extract a complete token from finished output (process EOF, or a test/paste
/// buffer). Lenient: accepts a token even if it runs to the end of the input.
pub fn extract_setup_token(output: &str) -> Option<String> {
    find_token(output, false)
}

/// Like [`extract_setup_token`] but for a *streaming* buffer: returns `None` if
/// the token runs to the very end of `output`, since more bytes may still be
/// coming and we'd otherwise capture a truncated prefix. The caller retries on
/// the next chunk once a terminating byte (newline, box border, …) lands.
pub fn extract_streaming_token(output: &str) -> Option<String> {
    find_token(output, true)
}

/// Extract the consent URL `claude` prints (the `oauth/authorize` link). Returns
/// the first `https://` run pointing at claude/oauth; `None` if none present.
pub fn find_consent_url(output: &str) -> Option<String> {
    let s = strip_ansi(output);
    let bytes = s.as_bytes();
    let needle = b"https://";
    let mut from = 0;
    while let Some(rel) = find_subslice(&bytes[from..], needle) {
        let start = from + rel;
        let mut end = start;
        while end < bytes.len() {
            let c = bytes[end];
            if c.is_ascii_whitespace() || c == b'"' || c == b'\'' || c == 0x07 {
                break;
            }
            end += 1;
        }
        let url = s[start..end].trim_end_matches(['.', ',', ')']).to_string();
        if url.contains("oauth") || url.contains("claude.") || url.contains("anthropic.") {
            return Some(url);
        }
        from = end.max(start + needle.len());
    }
    None
}

/// Whether the CLI is now sitting on its auth-code stdin prompt. The prompt
/// ("Paste code here if prompted >") is drawn with cursor-column escapes, so
/// after stripping ANSI the words collapse together — we match the collapsed
/// form to stay robust to that spacing and to line wrapping.
pub fn awaiting_code(output: &str) -> bool {
    let collapsed: String = strip_ansi(output).split_whitespace().collect();
    collapsed.contains("Pastecode")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::docker::auth::normalize_token;

    #[test]
    fn strip_ansi_removes_csi_osc_and_two_byte_escapes() {
        // ESC7 / ESC8 (two-byte), a CSI cursor move, and an OSC title with BEL.
        let raw = "\x1b7\x1b[9GWelcome\x1b]0;title\x07 to\x1b[0m Claude";
        assert_eq!(strip_ansi(raw), "Welcome to Claude");
    }

    #[test]
    fn strip_ansi_preserves_multibyte_content() {
        // Braille/block spinner glyphs must survive byte-level escape stripping.
        assert_eq!(strip_ansi("\x1b[2K✳ ░█▓"), "✳ ░█▓");
    }

    #[test]
    fn extract_token_from_a_clean_line() {
        let out = "Your token:\nsk-ant-oat01-Abc_123-XYZ\nDone.\n";
        assert_eq!(
            extract_setup_token(out).as_deref(),
            Some("sk-ant-oat01-Abc_123-XYZ")
        );
    }

    #[test]
    fn extract_token_from_a_noisy_ink_line() {
        // The token embedded amid cursor moves and a redraw, as Ink emits it.
        let out = "\x1b[2G\x1b[1mToken\x1b[7Gsk-ant-oat01-tok_EN-42\x1b[0m\r\n\x1b[?25h";
        assert_eq!(
            extract_setup_token(out).as_deref(),
            Some("sk-ant-oat01-tok_EN-42")
        );
    }

    #[test]
    fn extract_token_returns_none_on_unrelated_output() {
        // The consent URL / spinner frames carry no token.
        let out = "\x1b[2G· Opening browser\nhttps://claude.com/cai/oauth/authorize?code=true&state=abc\n";
        assert_eq!(extract_setup_token(out), None);
    }

    #[test]
    fn unexpected_shape_is_extracted_but_flagged_unrecognized() {
        // A non-`sk-ant-oat` shape (still `sk-ant-`) is captured; normalize_token
        // then flags it so the store path warns-but-stores (auth.rs behavior).
        let out = "token: sk-ant-api03-notanoauth\n";
        let token = extract_setup_token(out).expect("anchored on sk-ant-");
        assert_eq!(token, "sk-ant-api03-notanoauth");
        let (normalized, recognized) = normalize_token(&token).unwrap();
        assert_eq!(normalized, "sk-ant-api03-notanoauth");
        assert!(!recognized, "sk-ant-api… is not the recognized oat shape");
    }

    #[test]
    fn recognized_shape_passes_normalize() {
        let token = extract_setup_token("sk-ant-oat01-Good_Tok-9\n").unwrap();
        let (_, recognized) = normalize_token(&token).unwrap();
        assert!(recognized);
    }

    #[test]
    fn streaming_token_waits_for_a_terminator() {
        // Mid-stream: the run reaches the buffer edge, so hold off.
        assert_eq!(extract_streaming_token("...sk-ant-oat01-partial"), None);
        // Next chunk adds a terminating byte → now safe to capture.
        assert_eq!(
            extract_streaming_token("...sk-ant-oat01-partialX\n").as_deref(),
            Some("sk-ant-oat01-partialX")
        );
    }

    #[test]
    fn bare_anchor_is_not_a_token() {
        assert_eq!(extract_setup_token("sk-ant- and nothing else"), None);
    }

    #[test]
    fn find_consent_url_reconstructs_the_authorize_link() {
        // Wide-PTY output: the URL is one unbroken line after ANSI strip.
        let out = "\x1b[2GBrowser didn't open?\r\nhttps://claude.com/cai/oauth/authorize?code=true&client_id=abc&scope=user%3Ainference&state=xyz\r\n\x1b[2GPaste code here >";
        assert_eq!(
            find_consent_url(out).as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true&client_id=abc&scope=user%3Ainference&state=xyz")
        );
    }

    #[test]
    fn find_consent_url_ignores_non_oauth_urls() {
        assert_eq!(
            find_consent_url("see https://example.com/docs for help"),
            None
        );
    }

    #[test]
    fn awaiting_code_detects_the_collapsed_prompt() {
        // As emitted by Ink (column-positioned words → no spaces after strip).
        assert!(awaiting_code(
            "\x1b[2GPaste\x1b[8Gcode\x1b[13Ghere\x1b[18Gif\x1b[21Gprompted\x1b[30G>"
        ));
        // Also robust to normal spacing / wrapping.
        assert!(awaiting_code("Paste code here if prompted >"));
        assert!(!awaiting_code("Opening browser to sign in…"));
    }
}
