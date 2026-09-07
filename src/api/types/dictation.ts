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

/** Identifies one dictation session: the value `dictationStart` resolved with,
 *  stamped on every event of that session. Events are app-wide and a session
 *  outlives the composer that started it (a stop is followed by a flush), so a
 *  consumer must drop events whose `session` isn't the one it owns — otherwise
 *  the previous session's final transcript lands in the next composer's box. */
export type DictationSessionId = number;

/** Payload of the `dictation:transcript` event. `text` is the whole running
 *  transcript for that session (Apple revises earlier words as it hears more),
 *  not a delta — the UI replaces the in-progress segment with it. `is_final`
 *  marks the last result of a session, after which no more transcript events
 *  arrive for it. */
export interface DictationTranscriptEvent {
  session: DictationSessionId;
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
  session: DictationSessionId;
  state: DictationState;
  error: string | null;
}

/** The one Whisper model the local engine would use, as pinned in the Rust
 *  catalog (`dictation/whisper/models.rs`). */
export interface DictationModel {
  id: string;
  label: string;
  /** One line on when to pick it. */
  note: string;
  /** Exact download size in bytes. Every size string in the UI is derived from
   *  it, so the copy can't drift from the pinned file. */
  size: number;
}

/** The local (Whisper) engine's opt-in and the state of its weights. `enabled`
 *  and `installed` move independently: the engine can be chosen while the
 *  download is still running, in which case dictation keeps using the platform
 *  recognizer. */
export interface DictationModelStatus {
  enabled: boolean;
  installed: boolean;
  /** A download is running right now; watch `onDictationModelProgress`. */
  downloading: boolean;
  model: DictationModel;
}

/** `verifying` is the tail of the download (the digest is computed as bytes
 *  arrive), not a second pass — so it always follows a full `received`. */
export type DictationModelState = "downloading" | "verifying" | "installed" | "error";

/** Payload of the `dictation:model_progress` event. `total` is the pinned
 *  catalog size, not the server's `Content-Length`. `error` is set for the
 *  `error` state alone and is shown as-is. */
export interface DictationModelProgressEvent {
  model_id: string;
  state: DictationModelState;
  received: number;
  total: number | null;
  error: string | null;
}
