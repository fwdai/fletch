import { invoke } from "../invoke";
import type {
  DictationAvailability,
  DictationModelStatus,
  DictationSessionId,
} from "../types/dictation";

export const dictationApi = {
  /** Whether native dictation exists on this platform and what the user has
   *  authorized so far. Cheap; safe to call on every composer mount. */
  dictationAvailability: () => invoke<DictationAvailability>("dictation_availability"),
  /** Start listening. Requests whatever permission the chosen engine needs on
   *  first use — mic + speech for `apple`, mic alone for `whisper`. Rejects
   *  with a message if permission is denied or the recognizer can't start.
   *
   *  Resolves with the session id once audio is flowing: `dictation:state`
   *  `listening` has been emitted, transcripts follow via
   *  `onDictationTranscript`, and a terminal state is guaranteed — all stamped
   *  with that id, which the caller must match against (see
   *  `DictationSessionId`). Resolves `null` when nothing was started and no
   *  event will arrive — either a session was already active (only one runs at
   *  a time) or a `dictationStop` issued while a permission prompt was up
   *  cancelled this one. Callers must not wait for an event on `null`. */
  dictationStart: () => invoke<DictationSessionId | null>("dictation_start"),
  /** Stop listening and let the recognizer flush its final result: normally
   *  one last `dictation:transcript` with `is_final: true`, then
   *  `dictation:state` `stopped`. The final transcript is best-effort — a
   *  recognizer that hasn't flushed within a couple of seconds is torn down
   *  and only `stopped` arrives — so treat `stopped` as the point to commit
   *  whatever text was last received. The `whisper` engine has nothing to
   *  flush and everything to compute: it emits `transcribing` first, then the
   *  one final transcript, then `stopped`. No-op when not listening. */
  dictationStop: () => invoke<void>("dictation_stop"),

  /** The local (Whisper) engine's opt-in plus the state of its weights. Cheap
   *  — a metadata stat — so the Settings pane calls it on mount. */
  dictationModelStatus: () => invoke<DictationModelStatus>("dictation_model_status"),
  /** Pick the dictation engine. Persists `dictation_engine` and, when enabling
   *  without the weights on disk, starts the download in the background —
   *  resolves immediately either way, with progress arriving via
   *  `onDictationModelProgress`. Backend-owned, like `setCodeIndexingEnabled`. */
  setDictationEngine: (enabled: boolean) => invoke<void>("set_dictation_engine", { enabled }),
  /** Retry a failed (or never-started) model download. Resolves at once with
   *  the state the call left things in; a no-op while one is already running. */
  dictationModelDownload: () => invoke<DictationModelStatus>("dictation_model_download"),
  /** Delete the downloaded weights. Turn the engine off first — an enabled
   *  engine with no model silently falls back to the platform recognizer. */
  dictationModelRemove: () => invoke<DictationModelStatus>("dictation_model_remove"),
};
