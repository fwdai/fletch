import { invoke } from "../invoke";
import type { DictationAvailability, DictationSessionId } from "../types/dictation";

export const dictationApi = {
  /** Whether native dictation exists on this platform and what the user has
   *  authorized so far. Cheap; safe to call on every composer mount. */
  dictationAvailability: () => invoke<DictationAvailability>("dictation_availability"),
  /** Start listening. Requests mic + speech permission on first use. Rejects
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
   *  whatever text was last received. No-op when not listening. */
  dictationStop: () => invoke<void>("dictation_stop"),
};
