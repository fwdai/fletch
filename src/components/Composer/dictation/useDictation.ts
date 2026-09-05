import { useEffect, useRef, useState } from "react";
import { api, type DictationAvailability, onDictationState, onDictationTranscript } from "@/api";
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
};

/** Voice dictation for one composer: owns the session state and pipes the
 *  native recognizer's running transcript into the textarea.
 *
 *  Every `dictation:transcript` carries the WHOLE transcript of the session
 *  (Apple revises earlier words as it hears more), so the in-progress segment
 *  is REPLACED on each event — see [`spliceTranscript`]. The text the user had
 *  typed when they hit the mic is the `base` that segment is spliced onto, and
 *  it lives in a ref: a partial result lands several times a second and must
 *  not re-render anything but the textarea's value. */
export function useDictation(input: ComposerInput) {
  const [availability, setAvailability] = useState<DictationAvailability | null>(null);
  const [listening, setListening] = useState(false);
  // The backend keeps the recognizer claimed after a stop until its final
  // result lands (or the flush deadline passes), and starts nothing in that
  // window. Holding the control until the session actually ends is what keeps a
  // quick second click from lighting the mic with nothing behind it.
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // The text present when this session started; the transcript is spliced onto
  // it. Re-based when the user edits mid-session (see the effect below).
  const baseRef = useRef("");
  // The exact value we last wrote into the box. Distinguishes our own writes
  // from the user's edits, and guards against a late flush landing in a box
  // that has since been cleared (send) or rewritten.
  const lastWrittenRef = useRef("");
  // Read inside the event listeners, which are registered once for the
  // component's lifetime and so can't close over render-scoped values.
  const listeningRef = useRef(false);
  // Whether a transcript may still be written into the box. Outlives
  // `listening` by one event: `dictation_stop` is followed by the recognizer's
  // final result (punctuation and last revisions), which we do want. Closed by
  // that result, and by the box changing hands after the session ended.
  const acceptingRef = useRef(false);
  // True between `dictation_start` being called and resolving — the window the
  // OS permission prompts occupy on first use. An edit made then is the user
  // getting ahead of the mic, not the box changing hands, so it re-bases the
  // session instead of closing it.
  const startingRef = useRef(false);
  // Set when a pending start is no longer wanted — the composer unmounted, or
  // the message went out from under it. Every path that sets it also sends the
  // backend a stop, so this only decides what the start's continuation does
  // with whatever it gets back: never adopt it.
  const abortStartRef = useRef(false);
  const stoppingRef = useRef(false);
  // Set while this hook owns the one app-wide session, from the moment it asks
  // for a start until that session ends. `dictation:state` is a global event,
  // so without this a straggler from the session a previously mounted composer
  // left flushing would reset the one this hook just started.
  const ownsRef = useRef(false);
  // False once unmounted, so a probe (or a start) that settles late doesn't set
  // state on a gone component.
  const mountedRef = useRef(true);
  const inputRef = useRef(input);
  inputRef.current = input;

  function setListeningState(next: boolean) {
    listeningRef.current = next;
    setListening(next);
  }

  function setStoppingState(next: boolean) {
    stoppingRef.current = next;
    setStopping(next);
  }

  function probe() {
    api
      .dictationAvailability()
      .then((a) => {
        if (mountedRef.current) setAvailability(a);
      })
      .catch(() => {
        if (mountedRef.current) setAvailability(UNAVAILABLE);
      });
  }

  // Probe on mount, and again after every start attempt settles (see `toggle`):
  // the first-run prompt and a trip to System Settings both move the answer, and
  // a stale one leaves the wrong icon and tooltip until the composer remounts.
  // biome-ignore lint/correctness/useExhaustiveDependencies: probe on mount only; it reads nothing from render scope
  useEffect(() => {
    mountedRef.current = true;
    probe();
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // One subscription pair for the component's lifetime. The listener factories
  // resolve to their unlisten fn, so a mount that unmounts mid-await has to
  // unsubscribe on arrival (same pattern as the store's listener wiring).
  // biome-ignore lint/correctness/useExhaustiveDependencies: subscribe once; live values are read through refs
  useEffect(() => {
    let cancelled = false;
    let offTranscript: (() => void) | null = null;
    let offState: (() => void) | null = null;

    (async () => {
      const unTranscript = await onDictationTranscript((e) => {
        if (!acceptingRef.current) return;
        if (e.is_final) acceptingRef.current = false;
        const ip = inputRef.current;
        const { text, caret } = spliceTranscript(baseRef.current, e.text);
        lastWrittenRef.current = text;
        ip.setText(text);
        // Same tail as `append`: the box has to grow with the transcript and
        // keep the caret behind the last dictated word so typing continues there.
        requestAnimationFrame(() => {
          const el = ip.ta.current;
          if (!el) return;
          el.focus();
          grow(el);
          el.setSelectionRange(caret, caret);
        });
      });
      if (cancelled) {
        unTranscript();
        return;
      }
      offTranscript = unTranscript;

      const unState = await onDictationState((e) => {
        // Someone else's session (see `ownsRef`) — the event is app-wide, but
        // the state it reports isn't ours to act on.
        if (!ownsRef.current) return;
        if (e.state === "listening") {
          setListeningState(true);
          return;
        }
        // `stopped` and `error` both end the session; only `error` has a reason
        // worth showing (the backend's message doubles as the fix instruction).
        // Either one means teardown is done, so the mic is startable again.
        ownsRef.current = false;
        setListeningState(false);
        setStoppingState(false);
        acceptingRef.current = false;
        if (e.state === "error") setError(e.error ?? "Dictation failed");
      });
      if (cancelled) {
        unState();
        return;
      }
      offState = unState;
    })();

    return () => {
      cancelled = true;
      offTranscript?.();
      offState?.();
      acceptingRef.current = false;
      // Unmounting mid-session (a view switch) must not leave the mic open —
      // nothing is left to receive its transcript. A start still sitting on the
      // permission prompt is stopped too: the backend cancels it before the mic
      // ever opens, or stops the session if it came up first (see `stop`).
      if (startingRef.current) abortStartRef.current = true;
      if (startingRef.current || listeningRef.current) {
        listeningRef.current = false;
        void api.dictationStop().catch(() => {});
      }
    };
  }, []);

  // Runs after every committed change to the box, including our own writes —
  // so a mismatch against `lastWritten` is the user's own edit, whether they
  // typed it or the composer cleared the box on send.
  useEffect(() => {
    // The failure was about the last attempt, not about the text; typing
    // dismisses it.
    setError(null);
    if (input.text === lastWrittenRef.current) return;
    if (listeningRef.current || startingRef.current) {
      // Editing mid-session re-bases it: the next partial carries the whole
      // transcript again, and must replace the dictated tail, not their edit.
      baseRef.current = input.text;
      lastWrittenRef.current = input.text;
    } else {
      // The box changed hands after the session ended (send clears it) — the
      // recognizer's pending final result no longer belongs anywhere.
      acceptingRef.current = false;
    }
  }, [input.text]);

  /** End the session, whatever stage it's at — including a start still waiting
   *  on the OS permission prompt, which the backend cancels before the mic
   *  opens. A no-op when the mic is idle, so callers that just want it quiet
   *  (send) don't have to check first.
   *
   *  Optimistic about `listening`: the mic shouldn't stay lit while the final
   *  result is flushed — `dictation:state` `stopped` confirms the end. */
  function stop() {
    if (!listeningRef.current && !startingRef.current) return;
    if (startingRef.current) {
      // The start hasn't resolved yet, so send the stop anyway: the backend
      // honours one issued while a permission prompt is up and the mic never
      // opens. Both ends run on its main thread, so the request lands either
      // before the session is built (cancelled, the start resolves `false`) or
      // after (a real stop, terminal event to follow) — never in between. The
      // flag tells the start's continuation not to adopt what it opened.
      abortStartRef.current = true;
      acceptingRef.current = false;
    }
    setListeningState(false);
    setStoppingState(true);
    void api.dictationStop().catch(() => {
      // A failed stop emits no `stopped`, so nothing else would release the mic.
      ownsRef.current = false;
      setStoppingState(false);
    });
  }

  async function toggle() {
    if (listeningRef.current) {
      stop();
      return;
    }
    // A start still on the permission prompt, or a session still tearing down.
    // The backend starts nothing in either window, and a second attempt would
    // take over the refs the one in flight still needs. (The control is disabled
    // while stopping; this also catches a raced click.)
    if (startingRef.current || stoppingRef.current) return;
    baseRef.current = input.text;
    lastWrittenRef.current = input.text;
    acceptingRef.current = true;
    startingRef.current = true;
    abortStartRef.current = false;
    ownsRef.current = true;
    try {
      // Resolves once audio is flowing; on first use this is where the OS
      // permission prompts appear, so it can sit pending for a while.
      const started = await api.dictationStart();
      if (!started) {
        // Nothing came up — a stop of ours landed while the prompts were up, or
        // the backend was already busy. No terminal event is coming, so this is
        // the only place the control can be released.
        ownsRef.current = false;
        acceptingRef.current = false;
        setListeningState(false);
        setStoppingState(false);
        return;
      }
      if (abortStartRef.current) {
        // A stop was requested while this start was in flight, and it reached
        // the backend after the session came up — so it stopped that session for
        // real and the terminal event is on its way. Don't adopt what we're
        // about to be told is over; just leave the control held until it lands.
        setListeningState(false);
        return;
      }
      setError(null);
      setListeningState(true);
    } catch (e) {
      // Nothing was started, so no event will release the control from here.
      ownsRef.current = false;
      acceptingRef.current = false;
      setListeningState(false);
      setStoppingState(false);
      // The start's owner is gone; there's nothing to report the failure to.
      if (!abortStartRef.current) setError(String(e));
    } finally {
      startingRef.current = false;
      // The attempt may have settled a permission (the first-run prompt), which
      // decides the icon and its tooltip.
      probe();
    }
  }

  return { availability, listening, stopping, error, toggle, stop };
}
