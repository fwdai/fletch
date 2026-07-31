//! In-app feedback submission (the sidebar footer's "Send feedback" modal).
//!
//! Feedback rides the existing PostHog pipeline as a `feedback_submitted` event
//! — same transport as usage telemetry, but consent-independent and awaited; see
//! the carve-out documented in `crate::telemetry`. See `docs/feedback.md` for the
//! event contract and how to get notified when one arrives.

use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::telemetry;

/// Longest message we forward. PostHog drops any event over 1 MB, and this is
/// far more prose than a feedback box invites — a longer message is truncated
/// (with `message_truncated` set) rather than rejected, so nobody loses a wall
/// of text to a validation error.
const MAX_MESSAGE_CHARS: usize = 5_000;

/// Ceiling on the base64 screenshot. The frontend downscales and JPEG-encodes to
/// well under this (`src/util/image.ts`), so hitting it means something bypassed
/// that path — reject rather than have PostHog silently drop the whole event at
/// its 1 MB limit.
const MAX_SCREENSHOT_B64: usize = 600 * 1024;

/// Assemble the event properties. Pure and fallible, kept out of the command so
/// the trimming/capping rules are directly testable.
fn build_feedback_props(
    message: &str,
    contact_email: Option<&str>,
    screenshot_b64: Option<&str>,
    source: &str,
) -> Result<Value> {
    let message = message.trim();
    if message.is_empty() {
        return Err(Error::Other("Write a message before sending.".into()));
    }

    let mut props = Map::new();
    // Char-based, not byte-based: the cap exists to bound the payload, and
    // slicing mid-codepoint would panic.
    let truncated = message.chars().count() > MAX_MESSAGE_CHARS;
    let body: String = message.chars().take(MAX_MESSAGE_CHARS).collect();
    props.insert("message_chars".into(), json!(body.chars().count()));
    props.insert("message".into(), json!(body));
    if truncated {
        props.insert("message_truncated".into(), json!(true));
    }

    // Only present when the user left an address in the field — an empty or
    // whitespace-only value is an absent one, not an empty string on the event.
    if let Some(email) = contact_email.map(str::trim).filter(|e| !e.is_empty()) {
        props.insert("contact_email".into(), json!(email));
    }

    let screenshot = screenshot_b64.map(str::trim).filter(|s| !s.is_empty());
    if let Some(data) = screenshot {
        if data.len() > MAX_SCREENSHOT_B64 {
            return Err(Error::Other(
                "That screenshot is too large to send. Try a smaller crop.".into(),
            ));
        }
        props.insert("screenshot_jpeg_base64".into(), json!(data));
    }
    props.insert("has_screenshot".into(), json!(screenshot.is_some()));
    props.insert("source".into(), json!(source));

    Ok(Value::Object(props))
}

/// Send one piece of user feedback. Awaits the network so the modal can show a
/// real error (and offer its mailto fallback) instead of a fake success.
#[tauri::command]
pub async fn submit_feedback(
    message: String,
    contact_email: Option<String>,
    screenshot_base64: Option<String>,
    source: Option<String>,
) -> Result<()> {
    let props = build_feedback_props(
        &message,
        contact_email.as_deref(),
        screenshot_base64.as_deref(),
        source.as_deref().unwrap_or("sidebar"),
    )?;
    telemetry::send_now("feedback_submitted", props)
        .await
        .map_err(Error::Other)
}

#[cfg(test)]
mod feedback_props_tests {
    use super::*;

    fn props(v: &Value, key: &str) -> Option<Value> {
        v.get(key).cloned()
    }

    #[test]
    fn trims_the_message_and_counts_chars() {
        let p = build_feedback_props("  the sidebar is great  ", None, None, "sidebar").unwrap();
        assert_eq!(props(&p, "message"), Some(json!("the sidebar is great")));
        assert_eq!(props(&p, "message_chars"), Some(json!(20)));
        assert_eq!(
            props(&p, "message_truncated"),
            None,
            "a short message must not be flagged as truncated"
        );
        assert_eq!(props(&p, "source"), Some(json!("sidebar")));
    }

    #[test]
    fn rejects_a_blank_message() {
        assert!(build_feedback_props("   \n\t ", None, None, "sidebar").is_err());
        assert!(build_feedback_props("", None, None, "sidebar").is_err());
    }

    #[test]
    fn truncates_an_overlong_message_rather_than_failing() {
        let long = "x".repeat(MAX_MESSAGE_CHARS + 500);
        let p = build_feedback_props(&long, None, None, "sidebar").unwrap();
        assert_eq!(props(&p, "message_chars"), Some(json!(MAX_MESSAGE_CHARS)));
        assert_eq!(props(&p, "message_truncated"), Some(json!(true)));
    }

    #[test]
    fn truncates_on_char_boundaries() {
        // Multi-byte chars would panic on a byte slice; assert the cap counts
        // characters and the result is still valid UTF-8 of the right length.
        let long = "é".repeat(MAX_MESSAGE_CHARS + 10);
        let p = build_feedback_props(&long, None, None, "sidebar").unwrap();
        let message = p.get("message").and_then(Value::as_str).unwrap();
        assert_eq!(message.chars().count(), MAX_MESSAGE_CHARS);
    }

    #[test]
    fn omits_a_blank_contact_email() {
        let p = build_feedback_props("hi", Some("   "), None, "sidebar").unwrap();
        assert_eq!(props(&p, "contact_email"), None);

        let p = build_feedback_props("hi", Some(" ada@example.com "), None, "sidebar").unwrap();
        assert_eq!(props(&p, "contact_email"), Some(json!("ada@example.com")));
    }

    #[test]
    fn flags_a_screenshot_only_when_one_is_attached() {
        let p = build_feedback_props("hi", None, None, "sidebar").unwrap();
        assert_eq!(props(&p, "has_screenshot"), Some(json!(false)));
        assert_eq!(props(&p, "screenshot_jpeg_base64"), None);

        let p = build_feedback_props("hi", None, Some("Zm9v"), "sidebar").unwrap();
        assert_eq!(props(&p, "has_screenshot"), Some(json!(true)));
        assert_eq!(props(&p, "screenshot_jpeg_base64"), Some(json!("Zm9v")));

        // An empty string is no screenshot, not a zero-byte one.
        let p = build_feedback_props("hi", None, Some(""), "sidebar").unwrap();
        assert_eq!(props(&p, "has_screenshot"), Some(json!(false)));
    }

    #[test]
    fn rejects_an_oversize_screenshot() {
        let huge = "A".repeat(MAX_SCREENSHOT_B64 + 1);
        assert!(build_feedback_props("hi", None, Some(&huge), "sidebar").is_err());
    }
}
