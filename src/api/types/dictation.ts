// Voice dictation: the composer's mic button, backed by the platform's native
// speech recognizer (Apple's SFSpeechRecognizer on macOS/iOS). Mirrors the Rust
// `dictation` module's serde shapes — keep the two in sync.

/** Apple's authorization states for the mic and for speech recognition.
 *  `not_determined` means the OS hasn't asked the user yet; the first
 *  `dictation_start` triggers the prompt. `restricted` is a parental-control /
 *  MDM lock the user can't lift from the app. */
export type DictationAuthorization = "not_determined" | "authorized" | "denied" | "restricted";

export interface DictationAvailability {
  /** False on platforms with no native recognizer (Linux, Windows). The
   *  composer hides the mic button entirely when this is false. */
  supported: boolean;
  speech: DictationAuthorization;
  microphone: DictationAuthorization;
  /** The recognizer for the current locale can run without sending audio to
   *  Apple. When false, recognition uses Apple's servers and sessions are
   *  capped at roughly one minute. */
  on_device: boolean;
}

/** Payload of the `dictation:transcript` event. `text` is the whole running
 *  transcript for the current session (Apple revises earlier words as it hears
 *  more), not a delta — the UI replaces the in-progress segment with it.
 *  `is_final` marks the last result of a session, after which no more
 *  transcript events arrive until the next `dictation_start`. */
export interface DictationTranscriptEvent {
  text: string;
  is_final: boolean;
}

export type DictationState = "listening" | "stopped" | "error";

/** Payload of the `dictation:state` event. `error` is a human-readable reason,
 *  set only when `state` is `error`, which only happens when a session that
 *  was already listening fails. Failures to get started at all (permission
 *  denied, no recognizer for the locale, the mic wouldn't open) reject
 *  `dictationStart` and emit no event, so the rejected promise — not this
 *  state — is what tells the UI a start didn't take. */
export interface DictationStateEvent {
  state: DictationState;
  error: string | null;
}
