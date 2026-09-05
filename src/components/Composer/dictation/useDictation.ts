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
  const inputRef = useRef(input);
  inputRef.current = input;

  function setListeningState(next: boolean) {
    listeningRef.current = next;
    setListening(next);
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
        setListeningState(false);
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
      // nothing is left to receive its transcript.
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
    if (listeningRef.current) {
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

  /** Stop the recognizer. Optimistic: the mic shouldn't stay lit while the
   *  final result is flushed — `dictation:state` `stopped` confirms it. */
  function stop() {
    setListeningState(false);
    void api.dictationStop().catch(() => {});
  }

  async function toggle() {
    if (listeningRef.current) {
      stop();
      return;
    }
    baseRef.current = input.text;
    lastWrittenRef.current = input.text;
    acceptingRef.current = true;
    try {
      // Resolves once audio is flowing; on first use this is where the OS
      // permission prompts appear, so it can sit pending for a while.
      await api.dictationStart();
      setError(null);
      setListeningState(true);
    } catch (e) {
      setListeningState(false);
      acceptingRef.current = false;
      setError(String(e));
    }
  }

  return { availability, listening, error, toggle, stop };
}
