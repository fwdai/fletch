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
  // result lands (or the flush deadline passes), and answers a start in that
  // window with a successful no-op. Holding the control until the session
  // actually ends is what keeps a quick second click from lighting the mic with
  // nothing behind it.
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
  // the message went out from under it. `dictationStop` can't cancel a session
  // that hasn't come up yet, so the start closes it on arrival instead.
  const abortStartRef = useRef(false);
  const stoppingRef = useRef(false);
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

  // Probe once per mount: cheap, and the answer can change between mounts (the
  // user grants permission in System Settings while the app runs).
  useEffect(() => {
    let cancelled = false;
    api
      .dictationAvailability()
      .then((a) => {
        if (!cancelled) setAvailability(a);
      })
      .catch(() => {
        if (!cancelled) setAvailability(UNAVAILABLE);
      });
    return () => {
      cancelled = true;
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
        if (e.state === "listening") {
          setListeningState(true);
          return;
        }
        // `stopped` and `error` both end the session; only `error` has a reason
        // worth showing (the backend's message doubles as the fix instruction).
        // Either one means teardown is done, so the mic is startable again.
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
      // permission prompt has no session to stop yet, so it is marked unwanted
      // and shuts itself down the moment it comes up.
      if (startingRef.current) abortStartRef.current = true;
      if (listeningRef.current) {
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

  /** End the session, whatever stage it's at. A no-op when the mic is idle, so
   *  callers that just want it quiet (send) don't have to check first.
   *
   *  Optimistic about `listening`: the mic shouldn't stay lit while the final
   *  result is flushed — `dictation:state` `stopped` confirms the end. */
  function stop() {
    if (startingRef.current) {
      // Still on the permission prompt, so there's no session to stop and no
      // transcript worth keeping once it does come up.
      abortStartRef.current = true;
      acceptingRef.current = false;
    }
    if (!listeningRef.current) return;
    setListeningState(false);
    setStoppingState(true);
    void api.dictationStop().catch(() => {
      // A failed stop emits no `stopped`, so nothing else would release the mic.
      setStoppingState(false);
    });
  }

  async function toggle() {
    if (listeningRef.current) {
      stop();
      return;
    }
    // The button is held while the previous session tears down; this catches a
    // click that raced the disable.
    if (stoppingRef.current) return;
    baseRef.current = input.text;
    lastWrittenRef.current = input.text;
    acceptingRef.current = true;
    startingRef.current = true;
    abortStartRef.current = false;
    try {
      // Resolves once audio is flowing; on first use this is where the OS
      // permission prompts appear, so it can sit pending for a while.
      await api.dictationStart();
      if (abortStartRef.current) {
        // Nobody is left to dictate into — close the session that just opened
        // rather than leaving the mic live behind an idle button.
        void api.dictationStop().catch(() => {});
        return;
      }
      setError(null);
      setListeningState(true);
    } catch (e) {
      // The start's owner is gone; there's nothing to report the failure to.
      if (abortStartRef.current) return;
      setListeningState(false);
      acceptingRef.current = false;
      setError(String(e));
    } finally {
      startingRef.current = false;
    }
  }

  return { availability, listening, stopping, error, toggle, stop };
}
