import type { DictationPhase } from "@desktop/components/Composer/PrimaryControl/primaryState";
import { useCallback, useEffect, useRef, useState } from "react";
import type { DictationStatus } from "../api";
import { api, useStore } from "../store";
import { canCapture, startCapture } from "./capture";
import { LEVEL_BARS } from "./level";
import { DictationSession } from "./session";

export type { DictationPhase };

/** How long a failure shows in the composer's banner before it clears on its
 *  own. Same as the desktop's error pill. */
export const ERROR_TTL_MS = 2600;

const IDLE_LEVELS: number[] = Array(LEVEL_BARS).fill(0);

/** Voice dictation for one composer. Owns the session and reports its phase,
 *  the mic's loudness and when it opened; the transcript is handed to `onText`
 *  once, when the host answers.
 *
 *  Unlike the desktop hook there are no partials to preview — whisper answers
 *  once, at the end — so the composer only has to append.
 *
 *  A session usually ends itself: the mic reports the pause after the user
 *  stops talking (`silence.ts`), and that runs the same stop a tap would, so
 *  dictating is one tap — unless the Mac's "Stop after a pause" is off, which
 *  `dictationBegin` reports per session and `DictationSession` acts on, so a
 *  setting flipped on the Mac takes hold on the very next session rather than
 *  waiting for this phone to reconnect. Failures are the composer's to show
 *  (`error`), in a banner above the footer, and clear on their own. */
export function useDictation(onText: (text: string) => void) {
  const connection = useStore((s) => s.connection);
  // null while the probe is in flight; `available: false` with no reason is a
  // host too old to know the op, where no mic is best shown at all.
  const [status, setStatus] = useState<DictationStatus | null>(null);
  const [phase, setPhase] = useState<DictationPhase>("idle");
  // When the mic opened (epoch ms), for the clock. Null outside `listening`.
  const [startedAt, setStartedAt] = useState<number | null>(null);
  // The last `LEVEL_BARS` levels, oldest first.
  const [levels, setLevels] = useState<number[]>(IDLE_LEVELS);
  const [error, setError] = useState<string | null>(null);
  const sessionRef = useRef<DictationSession | null>(null);
  const phaseRef = useRef<DictationPhase>("idle");
  const errorTimer = useRef<number | null>(null);
  const onTextRef = useRef(onText);
  onTextRef.current = onText;

  const setPhaseState = useCallback((next: DictationPhase) => {
    phaseRef.current = next;
    setPhase(next);
  }, []);

  const fail = useCallback((e: unknown) => {
    setError(e instanceof Error ? e.message : String(e));
    if (errorTimer.current !== null) window.clearTimeout(errorTimer.current);
    errorTimer.current = window.setTimeout(() => {
      errorTimer.current = null;
      setError(null);
    }, ERROR_TTL_MS);
  }, []);

  const clearError = useCallback(() => {
    if (errorTimer.current !== null) window.clearTimeout(errorTimer.current);
    errorTimer.current = null;
    setError(null);
  }, []);

  // Probe on mount and on every reconnect: whether the Mac can transcribe at
  // all depends on its settings, which can change between one connection and
  // the next. Only what the mic button should look like rides on this — how a
  // session ends comes back from `dictationBegin`, per session, precisely so
  // that nothing about it is cached here between one and the next.
  useEffect(() => {
    if (connection !== "connected") return;
    let cancelled = false;
    api
      .dictationStatus()
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch(() => {
        if (!cancelled) setStatus({ available: false, reason: null });
      });
    return () => {
      cancelled = true;
    };
  }, [connection]);

  // Unmounting mid-session (leaving the agent) must not leave the mic open.
  useEffect(
    () => () => {
      void sessionRef.current?.cancel();
      sessionRef.current = null;
      if (errorTimer.current !== null) window.clearTimeout(errorTimer.current);
    },
    [],
  );

  /** The session is over, whatever the reason: rest the control. */
  const settle = useCallback(() => {
    setStartedAt(null);
    setLevels(IDLE_LEVELS);
    setPhaseState("idle");
  }, [setPhaseState]);

  /** Close the session and hand over what the Mac heard. The one stop path: a
   *  tap on the disc and the mic's own "done talking" both land here, and
   *  taking the session out of the ref claims it, so the two can't end it
   *  twice. */
  const finish = useCallback(async () => {
    const session = sessionRef.current;
    if (!session) return;
    sessionRef.current = null;
    setPhaseState("transcribing");
    setStartedAt(null);
    setLevels(IDLE_LEVELS);
    try {
      const text = await session.stop();
      if (text) onTextRef.current(text);
    } catch (e) {
      fail(e);
    } finally {
      settle();
    }
  }, [fail, settle, setPhaseState]);

  /** Back out: close the mic and throw the audio away. Nothing reaches the
   *  draft. */
  const cancel = useCallback(() => {
    const session = sessionRef.current;
    if (!session) return;
    sessionRef.current = null;
    settle();
    void session.cancel();
  }, [settle]);

  const start = useCallback(async () => {
    if (phaseRef.current !== "idle") return;
    if (status && !status.available) {
      // The disc is shown off; tapping it says why.
      if (status.reason) fail(new Error(status.reason));
      return;
    }
    clearError();
    setLevels(IDLE_LEVELS);
    setPhaseState("starting");
    const session = new DictationSession(api, startCapture);
    sessionRef.current = session;
    try {
      await session.start(
        () => void finish(),
        (level) => setLevels((prev) => [...prev.slice(1), level]),
      );
      // A cancel while the mic was opening already let go of the session.
      if (sessionRef.current !== session) return;
      setPhaseState("listening");
      setStartedAt(Date.now());
    } catch (e) {
      if (sessionRef.current === session) sessionRef.current = null;
      settle();
      fail(e);
    }
  }, [status, fail, clearError, finish, settle, setPhaseState]);

  /** The mic's own action: open it, or close it and transcribe. */
  const toggle = useCallback(async () => {
    if (phaseRef.current === "listening") await finish();
    else if (phaseRef.current === "idle") await start();
  }, [finish, start]);

  return {
    /** False until the host says it can transcribe, or when this webview has
     *  no microphone API; the composer offers no mic then. */
    supported: canCapture() && status !== null && (status.available || status.reason !== null),
    /** The host can't transcribe right now (engine off, model missing). The
     *  disc shows it; a tap surfaces the reason. */
    blocked: status !== null && !status.available,
    phase,
    startedAt,
    levels,
    error,
    toggle,
    start,
    cancel,
  };
}
