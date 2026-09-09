/** The composer's primary control is one pill in the footer's far-right slot
 *  that is, in turn, the microphone, the send arrow and the stop button. Which
 *  one it is right now is derived from the composer's inputs — the control
 *  stores nothing of its own. Evaluate top to bottom; the first match wins. */
export type PrimaryState =
  | "empty"
  | "draft"
  | "listening"
  | "transcribing"
  | "running"
  | "error"
  | "unavailable";

/** Where a dictation session is, as `useDictation` reports it. `starting` is the
 *  window between the click and audio flowing — the OS permission prompt lives
 *  there — and the control shows it as listening so the click answers at once.
 *  `transcribing` covers both the local engine running its model and Apple's
 *  post-stop flush, which is the same wait from the user's side. */
export type DictationPhase = "idle" | "starting" | "listening" | "transcribing";

export interface PrimaryInputs {
  /** A transcription just failed and the failure is still showing. */
  sttError: boolean;
  dictation: DictationPhase;
  /** The agent is working on a turn. */
  agentRunning: boolean;
  /** Text or attachments are staged. */
  hasDraft: boolean;
  /** The microphone grant is denied or restricted; only System Settings can
   *  undo it. Replaces `empty` alone — a draft or a run still shows normally. */
  micDenied: boolean;
}

export function primaryState(i: PrimaryInputs): PrimaryState {
  if (i.sttError) return "error";
  if (i.dictation === "listening" || i.dictation === "starting") return "listening";
  if (i.dictation === "transcribing") return "transcribing";
  if (i.agentRunning) return "running";
  if (i.hasDraft) return "draft";
  if (i.micDenied) return "unavailable";
  return "empty";
}

/** `m:ss` for the listening clock. */
export function formatClock(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}
