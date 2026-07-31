# In-app feedback

The speech-bubble button in the sidebar footer (beside the theme toggle) opens a
modal where a user writes a message, optionally leaves a reply-to address, and
optionally attaches a screenshot. This document is how you read what they sent.

## Where it goes

Feedback rides the **existing PostHog pipeline** as a single
`feedback_submitted` event. That was a deliberate choice: the transport already
exists (`src-tauri/src/telemetry.rs`), so there is no service to deploy, no
secret to rotate, and no new dependency. The trade-off is that feedback lands in
an analytics tool rather than an inbox — see [Getting
notified](#getting-notified) for closing that gap without writing code.

The flow, end to end:

| Layer | File |
| --- | --- |
| Button | `src/components/Sidebar/SidebarFooter.tsx` → `openFeedback()` (`src/store/ui.ts`) |
| Modal | `src/components/Feedback/` (shell, form, screenshot field, `useFeedback`) |
| Screenshot encode | `src/util/image.ts` — `pickImageAsBase64` |
| IPC | `api.submitFeedback` (`src/api/domains/misc.ts`) → `submit_feedback` |
| Command | `src-tauri/src/commands/feedback.rs` |
| Transport | `telemetry::send_now` (`src-tauri/src/telemetry.rs`) |

## The event

`feedback_submitted`, with these properties on top of the usual super-props
(`app_version`, `app_channel`, `os`, `arch`) and the anonymous
`distinct_id`:

| Property | Notes |
| --- | --- |
| `message` | The user's text, trimmed. Truncated at 5 000 characters. |
| `message_chars` | Length of what was actually sent. |
| `message_truncated` | Present (`true`) only when the cap bit. |
| `contact_email` | **Absent** unless the user left an address in the field. |
| `has_screenshot` | Always present. |
| `screenshot_jpeg_base64` | Base64 JPEG, only when one was attached. |
| `source` | Which surface opened the modal (`sidebar` today). |

**PostHog will not render the screenshot.** It arrives as a base64 string you
have to decode — paste it after `data:image/jpeg;base64,` in a browser address
bar, or have a destination (below) forward it somewhere that renders images.
This is the main cost of the no-infrastructure choice; a relay endpoint that
emails the attachment is the obvious upgrade, and `submit_feedback` is the single
place to add it.

## Why the size caps exist

PostHog **drops any single event over 1 MB**. A retina `⌘⇧4` screenshot is
routinely 3–5 MB, so an attachment can't be sent as-is:

- The webview downscales to a 1600px longest edge and JPEG-encodes it, walking
  down a quality ladder until the payload fits (`src/util/image.ts`). Typical
  result: 150–350 KB. Doing this in the webview means no image crate on the Rust
  side.
- `MAX_SCREENSHOT_B64` (600 KB) in `commands/feedback.rs` is the backstop, with
  headroom for the message and super-props. If it ever trips, something bypassed
  the encode path.
- Messages are capped at 5 000 characters and **truncated, not rejected** — a
  validation error should never cost someone a wall of text.

## Consent, and why feedback ignores it

`telemetry::track` is gated on the "anonymous usage telemetry" toggle
(Settings › General). Feedback uses `telemetry::send_now`, which **ignores that
gate**: turning off usage analytics should not silence a message the user
deliberately typed and pressed Send on. The modal states its destination in
plain text, and identity is still the anonymous per-install UUID — nothing calls
`$identify`, so a `contact_email` is one event property, never a person profile.

Feedback is still hard-gated on `QUORUM_POSTHOG_KEY` being baked in: an
unconfigured build (every dev build by default) has nowhere to send. It says so
rather than swallowing the message, and offers a prefilled
`mailto:feedback@fletch.sh` instead. Put a real key in `.env` (see
`.env.example`) to exercise the live path locally.

## Getting notified

Nothing in the app pushes feedback to a human — do this once in the PostHog UI:

1. **Data pipelines → Destinations → New destination** (Slack, or a generic
   webhook / email service).
2. Filter on event `feedback_submitted`.
3. Include `message`, `contact_email`, `app_version` and `os` in the payload.

Until that's wired up, feedback is only visible in PostHog's activity/events
view — worth checking before assuming nobody has sent any.
