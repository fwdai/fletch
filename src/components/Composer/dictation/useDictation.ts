import { useEffect, useRef, useState } from "react";
import {
  api,
  type DictationAvailability,
  type DictationSessionId,
  onDictationLevel,
  onDictationState,
  onDictationTranscript,
} from "@/api";
import type { DictationPhase } from "../PrimaryControl/primaryState";
import { type ComposerInput, grow } from "../useComposerInput";
import { spliceTranscript } from "./spliceTranscript";

/** What we assume when the availability probe itself fails — an older backend
 *  without the command, or a non-Tauri environment (tests, storybook-ish
 *  renders). Nothing is offered rather than a button that can only error. */
const UNAVAILABLE: DictationAvailability = {
  supported: false,
  speech: "not_determined",
  microphone: "not_determined",
  on_device: false,
  engine: "apple",
};

/** How long a failure shows before the control returns to idle on its own.
 *  Typing dismisses it sooner. */
export const ERROR_TTL_MS = 2600;

/** How long a just-committed span is marked, so the eye can find what just
 *  arrived. A hair longer than its CSS wash, which fades over 1.1 s. */
export const FRESH_TTL_MS = 1200;

/** How many recent microphone levels the waveform shows. */
export const LEVEL_BARS = 5;
const IDLE_LEVELS: number[] = Array(LEVEL_BARS).fill(0);

/** Offsets into the composer text of the span the last commit inserted. */
export interface FreshSpan {
  start: number;
  end: number;
}

/** The last probe's answer, so a composer that mounts after the first one
 *  starts with the right control instead of flashing the wrong one while its
 *  own probe is in flight. */
let lastAvailability: DictationAvailability | null = null;

/** Voice dictation for one composer: owns the session state, shows the
 *  recognizer's running transcript beside the text, and commits it once.
 *
 *  Every `dictation:transcript` carries the WHOLE transcript of the session
 *  (Apple revises earlier words as it hears more), so `interim` is REPLACED on
 *  each event. It is not written into the box: the composer renders it in a
 *  ghost layer over the textarea, so undo history stays clean and a cancel is
 *  a no-op on the draft. The final result is spliced onto whatever the box
 *  holds at that moment (see [`spliceTranscript`]) — the user may have kept
 *  typing — and the inserted span is reported as `fresh` for a moment. */
export function useDictation(input: ComposerInput) {
  const [availability, setAvailability] = useState<DictationAvailability | null>(lastAvailability);
  const [phase, setPhase] = useState<DictationPhase>("idle");
  const [interim, setInterim] = useState("");
  // When audio started flowing, for the clock. Null outside `listening`.
  const [startedAt, setStartedAt] = useState<number | null>(null);
  // The last `LEVEL_BARS` microphone levels, oldest first.
  const [levels, setLevels] = useState<number[]>(IDLE_LEVELS);
  const [error, setError] = useState<string | null>(null);
  const [fresh, setFresh] = useState<FreshSpan | null>(null);

  // Read inside the event listeners, which are registered once for the
  // component's lifetime and so can't close over render-scoped values.
  const phaseRef = useRef<DictationPhase>("idle");
  const interimRef = useRef("");
  // Whether a transcript may still be written. Outlives the mic by one event:
  // `dictation_stop` is followed by the recognizer's final result (punctuation
  // and last revisions), which we do want. Closed by that result, by a cancel,
  // and by the terminal state.
  const acceptingRef = useRef(false);
  // Whether this session's text has landed in the box (or was discarded), so
  // the terminal event knows whether to commit what was last heard — Apple's
  // flush deadline can pass without a final result.
  const committedRef = useRef(true);
  // Set when a pending start is no longer wanted — the composer unmounted, or
  // the user backed out while the permission prompt was up. Every path that
  // sets it also sends the backend a stop, so this only decides what the
  // start's continuation does with whatever it gets back: never adopt it.
  const abortStartRef = useRef(false);
  // The id of the session this hook owns, or null. Both dictation events are
  // app-wide and a session outlives the composer that started it (a stop is
  // followed by a flush), so a composer that starts the next one would otherwise
  // take the old session's final transcript into its box and let the old
  // `stopped` reset it. Every event is matched against this before it counts.
  // Set from the `listening` event while a start is in flight (so an early
  // event of our own is recognised), then authoritatively from what
  // `dictationStart` resolves with.
  const sessionRef = useRef<DictationSessionId | null>(null);
  // The last session of ours that ended. A terminal event can beat the start's
  // own reply (an instant recognizer failure); the reply must not re-adopt a
  // session the event already closed out.
  const endedRef = useRef<DictationSessionId | null>(null);
  // False once unmounted, so a probe (or a start) that settles late doesn't set
  // state on a gone component.
  const mountedRef = useRef(true);
  const inputRef = useRef(input);
  inputRef.current = input;
  // The exact value the last commit wrote, so the "typing dismisses the error"
  // effect can tell our own write from the user's edit.
  const lastWrittenRef = useRef<string | null>(null);
  const errorTimer = useRef<number | null>(null);
  const freshTimer = useRef<number | null>(null);

  function setPhaseState(next: DictationPhase) {
    phaseRef.current = next;
    setPhase(next);
  }

  /** The phase as of now. A plain read, but through a function so TypeScript
   *  doesn't carry a narrowing across the awaits in `start`. */
  const phaseNow = (): DictationPhase => phaseRef.current;

  function setInterimState(next: string) {
    interimRef.current = next;
    setInterim(next);
  }

  function fail(message: string) {
    setError(message);
    if (errorTimer.current !== null) window.clearTimeout(errorTimer.current);
    errorTimer.current = window.setTimeout(() => {
      errorTimer.current = null;
      if (mountedRef.current) setError(null);
    }, ERROR_TTL_MS);
  }

  /** Splice what the session heard onto the box. Once per session at most;
   *  nothing heard leaves the box untouched — no dangling space, no toast. */
  function commit(transcript: string) {
    committedRef.current = true;
    if (!transcript) return;
    const ip = inputRef.current;
    const { text, caret } = spliceTranscript(ip.text, transcript);
    lastWrittenRef.current = text;
    ip.setText(text);
    setFresh({ start: text.length - transcript.length, end: text.length });
    if (freshTimer.current !== null) window.clearTimeout(freshTimer.current);
    freshTimer.current = window.setTimeout(() => {
      freshTimer.current = null;
      if (mountedRef.current) setFresh(null);
    }, FRESH_TTL_MS);
    // Same tail as `append`: the box has to grow with the text and keep the
    // caret behind the last dictated word so typing continues there. Focus
    // returns to the field after every commit.
    requestAnimationFrame(() => {
      const el = ip.ta.current;
      if (!el) return;
      el.focus();
      grow(el);
      el.setSelectionRange(caret, caret);
    });
  }

  /** The session is over, whatever the reason: forget it and rest the control. */
  function settle() {
    sessionRef.current = null;
    acceptingRef.current = false;
    setInterimState("");
    setStartedAt(null);
    setLevels(IDLE_LEVELS);
    setPhaseState("idle");
  }

  function probe() {
    api
      .dictationAvailability()
      .then((a) => {
        lastAvailability = a;
        if (mountedRef.current) setAvailability(a);
      })
      .catch(() => {
        if (mountedRef.current) setAvailability(UNAVAILABLE);
      });
  }

  // Probe on mount, and again after every start attempt settles (see `start`):
  // the first-run prompt and a trip to System Settings both move the answer, and
  // a stale one leaves the wrong control until the composer remounts.
  // biome-ignore lint/correctness/useExhaustiveDependencies: probe on mount only; it reads nothing from render scope
  useEffect(() => {
    mountedRef.current = true;
    probe();
    return () => {
      mountedRef.current = false;
      if (errorTimer.current !== null) window.clearTimeout(errorTimer.current);
      if (freshTimer.current !== null) window.clearTimeout(freshTimer.current);
    };
  }, []);

  // One set of subscriptions for the component's lifetime. The listener
  // factories resolve to their unlisten fn, so a mount that unmounts mid-await
  // has to unsubscribe on arrival (same pattern as the store's listener wiring).
  // biome-ignore lint/correctness/useExhaustiveDependencies: subscribe once; live values are read through refs
  useEffect(() => {
    let cancelled = false;
    const offs: (() => void)[] = [];

    (async () => {
      const subscribe = async (make: () => Promise<() => void>) => {
        const off = await make();
        if (cancelled) off();
        else offs.push(off);
        return !cancelled;
      };

      const live = await subscribe(() =>
        onDictationTranscript((e) => {
          // A previous session's flush, or one we've since disowned — not ours
          // to show. A partial of our own arriving before the id is known is
          // safe to drop: the next one carries the whole transcript again.
          if (e.session !== sessionRef.current) return;
          if (!acceptingRef.current) return;
          if (e.is_final) {
            acceptingRef.current = false;
            setInterimState("");
            commit(e.text);
          } else {
            setInterimState(e.text);
          }
        }),
      );
      if (!live) return;

      const stillLive = await subscribe(() =>
        onDictationState((e) => {
          if (e.state === "listening") {
            // Only one session can come up at a time, so the `listening` that
            // lands while our start is in flight is ours: adopt its id here, so
            // a terminal event that beats the start's own reply is still matched.
            if (phaseRef.current === "starting" && sessionRef.current === null) {
              sessionRef.current = e.session;
            }
            if (e.session === sessionRef.current) {
              setPhaseState("listening");
              setStartedAt(Date.now());
            }
            return;
          }
          // Someone else's session (see `sessionRef`) — the event is app-wide,
          // but the state it reports isn't ours to act on.
          if (e.session !== sessionRef.current) return;
          if (e.state === "transcribing") {
            // Not terminal: the mic is off but the final transcript is still
            // coming. The bars have nothing left to show.
            setPhaseState("transcribing");
            setStartedAt(null);
            return;
          }
          // `stopped` and `error` both end the session; only `error` has a reason
          // worth showing. Either one means teardown is done, so the mic is
          // startable again. A session that ended without a final result (the
          // flush deadline, an error mid-utterance) still had text: commit what
          // was last heard rather than lose it.
          endedRef.current = e.session;
          if (acceptingRef.current && !committedRef.current) commit(interimRef.current);
          settle();
          if (e.state === "error") fail(e.error ?? "Dictation failed");
        }),
      );
      if (!stillLive) return;

      await subscribe(() =>
        onDictationLevel((e) => {
          if (e.session !== sessionRef.current) return;
          setLevels((prev) => [...prev.slice(1), e.level]);
        }),
      );
    })();

    return () => {
      cancelled = true;
      for (const off of offs) off();
      acceptingRef.current = false;
      // Unmounting mid-session (a view switch) must not leave the mic open —
      // nothing is left to receive its transcript. A start still sitting on the
      // permission prompt is stopped too: the backend cancels it before the mic
      // ever opens, or stops the session if it came up first (see `stop`).
      const ph = phaseRef.current;
      if (ph === "starting") abortStartRef.current = true;
      if (ph === "starting" || ph === "listening") {
        phaseRef.current = "idle";
        void api.dictationStop().catch(() => {});
      }
    };
  }, []);

  // The failure was about the last attempt, not about the text; typing
  // dismisses it. Our own commit doesn't count as typing — a session that
  // failed after hearing something commits and reports the error in the same
  // breath, and the error has to survive that write.
  useEffect(() => {
    if (input.text === lastWrittenRef.current) return;
    if (errorTimer.current !== null) {
      window.clearTimeout(errorTimer.current);
      errorTimer.current = null;
    }
    setError(null);
  }, [input.text]);

  /** Close the mic and let the recognizer finish: the final transcript lands in
   *  the box. Also ends a start still waiting on the OS permission prompt, which
   *  the backend cancels before the mic opens. A no-op when idle.
   *
   *  Optimistic about the phase: the flush is the same wait as the local
   *  engine's model run, so the control shows `transcribing` until the
   *  terminal `dictation:state` confirms the end. */
  function stop() {
    const ph = phaseRef.current;
    if (ph !== "listening" && ph !== "starting") return;
    if (ph === "starting") {
      // The start hasn't resolved yet, so send the stop anyway: the backend
      // honours one issued while a permission prompt is up and the mic never
      // opens. Both ends run on its main thread, so the request lands either
      // before the session is built (cancelled, the start resolves `null`) or
      // after (a real stop, terminal event to follow) — never in between. The
      // flag tells the start's continuation not to adopt what it opened.
      abortStartRef.current = true;
      acceptingRef.current = false;
    }
    setPhaseState("transcribing");
    setStartedAt(null);
    void api.dictationStop().catch(() => {
      // A failed stop emits no `stopped`, so nothing else would release the mic.
      settle();
    });
  }

  /** Back out: close the mic and discard whatever it heard. The draft is
   *  untouched — the interim text was never in it. */
  function cancel() {
    const ph = phaseRef.current;
    if (ph !== "listening" && ph !== "starting") return;
    acceptingRef.current = false;
    committedRef.current = true;
    setInterimState("");
    stop();
  }

  async function start() {
    // A start still on the permission prompt, or a session still ending. The
    // backend starts nothing in either window, and a second attempt would take
    // over the refs the one in flight still needs.
    if (phaseRef.current !== "idle") return;
    acceptingRef.current = true;
    committedRef.current = false;
    abortStartRef.current = false;
    sessionRef.current = null;
    setInterimState("");
    setLevels(IDLE_LEVELS);
    setError(null);
    setPhaseState("starting");
    try {
      // Resolves once audio is flowing; on first use this is where the OS
      // permission prompts appear, so it can sit pending for a while.
      const id = await api.dictationStart();
      if (id === null) {
        // Nothing came up — a stop of ours landed while the prompts were up, or
        // the backend was already busy. No terminal event is coming, so this is
        // the only place the control can be released.
        settle();
        return;
      }
      if (endedRef.current === id) {
        // Came up and ended before this reply arrived; the terminal event has
        // already released the control, and adopting the id now would hold it
        // for an event that isn't coming.
        return;
      }
      // Authoritative, whatever the `listening` handler adopted meanwhile.
      sessionRef.current = id;
      if (abortStartRef.current) {
        // A stop was requested while this start was in flight, and it reached
        // the backend after the session came up — so it stopped that session for
        // real and the terminal event is on its way. Don't show a live mic for
        // what we're about to be told is over; hold until it lands.
        setPhaseState("transcribing");
        return;
      }
      if (phaseNow() === "starting") {
        setPhaseState("listening");
        setStartedAt(Date.now());
      }
    } catch (e) {
      // Nothing was started, so no event will release the control from here.
      settle();
      // The start's owner is gone; there's nothing to report the failure to.
      if (!abortStartRef.current) fail(String(e));
    } finally {
      // The attempt may have settled a permission (the first-run prompt), which
      // decides the control and its tooltip.
      probe();
    }
  }

  /** The mic's own action: open it, or close it and transcribe. */
  function toggle() {
    if (phaseRef.current === "listening") stop();
    else if (phaseRef.current === "idle") void start();
  }

  /** The error pill's action: try again. */
  function retry() {
    if (errorTimer.current !== null) {
      window.clearTimeout(errorTimer.current);
      errorTimer.current = null;
    }
    setError(null);
    void start();
  }

  return {
    availability,
    phase,
    interim,
    startedAt,
    levels,
    error,
    fresh,
    toggle,
    stop,
    cancel,
    retry,
  };
}
