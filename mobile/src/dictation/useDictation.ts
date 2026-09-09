import { useCallback, useEffect, useRef, useState } from "react";
import type { DictationStatus } from "../api";
import { api, useStore } from "../store";
import { canCapture, startCapture } from "./capture";
import { DictationSession } from "./session";

export type DictationPhase = "idle" | "starting" | "listening" | "transcribing";

/** Failures land where every other action's do (`lastError`), so the composer
 *  needs no error UI of its own. */
const fail = (e: unknown) => {
  useStore.setState({ lastError: e instanceof Error ? e.message : String(e) });
};

/** Voice dictation for one composer. Owns the session and reports its phase;
 *  the transcript is handed to `onText` once, when the host answers.
 *
 *  Unlike the desktop hook there are no partials to splice as they arrive —
 *  whisper answers once, at the end — so the composer only has to append.
 *
 *  A session usually ends itself: the mic reports the pause after the user
 *  stops talking (`silence.ts`), and that runs the same stop a second tap
 *  would, so dictating is one tap. */
export function useDictation(onText: (text: string) => void) {
  const connection = useStore((s) => s.connection);
  // null while the probe is in flight; `available: false` with no reason is a
  // host too old to know the op, where the button is best not shown at all.
  const [status, setStatus] = useState<DictationStatus | null>(null);
  const [phase, setPhase] = useState<DictationPhase>("idle");
  const sessionRef = useRef<DictationSession | null>(null);
  const onTextRef = useRef(onText);
  onTextRef.current = onText;

  // Probe on mount and on every reconnect: the answer depends on the Mac's
  // settings, which can change between one connection and the next.
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
    },
    [],
  );

  /** Close the session and splice what the Mac heard. The one stop path: a tap
   *  on the button and the mic's own "done talking" both land here, and taking
   *  the session out of the ref claims it, so the two can't end it twice. */
  const finish = useCallback(async () => {
    const session = sessionRef.current;
    if (!session) return;
    sessionRef.current = null;
    setPhase("transcribing");
    try {
      const text = await session.stop();
      if (text) onTextRef.current(text);
    } catch (e) {
      fail(e);
    } finally {
      setPhase("idle");
    }
  }, []);

  const toggle = useCallback(async () => {
    if (phase === "listening") {
      await finish();
      return;
    }
    if (phase !== "idle") return;
    if (status && !status.available) {
      // The mic is shown slashed; tapping it says why.
      if (status.reason) fail(new Error(status.reason));
      return;
    }
    setPhase("starting");
    const session = new DictationSession(api, startCapture);
    sessionRef.current = session;
    try {
      await session.start(() => void finish());
      setPhase("listening");
    } catch (e) {
      sessionRef.current = null;
      setPhase("idle");
      fail(e);
    }
  }, [phase, status, finish]);

  return {
    /** False until the host says it can transcribe, or when this webview has
     *  no microphone API; the composer hides the button then. */
    supported: canCapture() && status !== null && (status.available || status.reason !== null),
    blocked: status !== null && !status.available,
    phase,
    toggle,
  };
}
