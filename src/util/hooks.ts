import { type DependencyList, type RefObject, useEffect, useState } from "react";

/** Dismiss a popover/menu on an outside pointer press or Escape, while `active`.
 *  Listens in the capture phase so it still fires when an inner handler stops
 *  propagation. `onDismiss` may be a fresh closure each render — the listeners
 *  re-subscribe, which is cheap. */
export function useDismiss(
  ref: RefObject<HTMLElement | null>,
  active: boolean,
  onDismiss: () => void,
) {
  useEffect(() => {
    if (!active) return;
    const onDown = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onDismiss();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onDismiss();
    };
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [active, ref, onDismiss]);
}

/** Re-render once a minute so age strings ("5m", "2h") stay fresh. */
export function useMinuteClock(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(id);
  }, []);
  return now;
}

/** Run `tick` immediately and then every `intervalMs` while mounted.
 *
 *  - Skip-if-in-flight: a slow tick can't stack with the next one.
 *  - Pauses while `document.hidden`; resumes (and re-runs once) on
 *    visibility return, so a backgrounded app doesn't burn polls.
 *  - `deps` is the React useEffect deps array — change it when the
 *    work `tick` captures changes (e.g. a different agent id).
 */
export function usePoll(tick: () => Promise<void>, intervalMs: number, deps: DependencyList) {
  useEffect(() => {
    let cancelled = false;
    let inFlight = false;
    let intervalId: ReturnType<typeof setInterval> | null = null;

    const run = async () => {
      if (cancelled || inFlight || document.hidden) return;
      inFlight = true;
      try {
        await tick();
      } finally {
        inFlight = false;
      }
    };
    const start = () => {
      if (intervalId != null) return;
      void run();
      intervalId = setInterval(run, intervalMs);
    };
    const stop = () => {
      if (intervalId == null) return;
      clearInterval(intervalId);
      intervalId = null;
    };
    const onVisibilityChange = () => {
      if (document.hidden) stop();
      else start();
    };

    if (!document.hidden) start();
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      cancelled = true;
      stop();
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intervalMs, ...deps]);
}
