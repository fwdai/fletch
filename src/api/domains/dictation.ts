import { invoke } from "../invoke";
import type { DictationAvailability } from "../types/dictation";

export const dictationApi = {
  /** Whether native dictation exists on this platform and what the user has
   *  authorized so far. Cheap; safe to call on every composer mount. */
  dictationAvailability: () => invoke<DictationAvailability>("dictation_availability"),
  /** Start listening. Requests mic + speech permission on first use. Resolves
   *  once audio is flowing; transcripts then arrive via `onDictationTranscript`.
   *  Rejects with a message if permission is denied or the recognizer can't
   *  start. Only one session runs at a time — a second start while listening
   *  is a no-op, and a `dictationStop` issued while a permission prompt is
   *  still up cancels the pending session, so this resolves with no
   *  `listening` state ever arriving. */
  dictationStart: () => invoke<void>("dictation_start"),
  /** Stop listening and let the recognizer flush its final result: normally
   *  one last `dictation:transcript` with `is_final: true`, then
   *  `dictation:state` `stopped`. The final transcript is best-effort — a
   *  recognizer that hasn't flushed within a couple of seconds is torn down
   *  and only `stopped` arrives — so treat `stopped` as the point to commit
   *  whatever text was last received. No-op when not listening. */
  dictationStop: () => invoke<void>("dictation_stop"),
};
