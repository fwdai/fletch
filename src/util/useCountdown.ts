import { useEffect, useState } from "react";

/** Seconds left before `iso` lapses, floored at zero. */
export function secondsLeft(iso: string): number {
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return 0;
  return Math.max(0, Math.floor((at - Date.now()) / 1000));
}

/** `secondsLeft(iso)`, ticking every second: how long a pairing code shown on
 *  screen is still good for. */
export function useCountdown(iso: string): number {
  const [left, setLeft] = useState(() => secondsLeft(iso));
  useEffect(() => {
    setLeft(secondsLeft(iso));
    const timer = setInterval(() => setLeft(secondsLeft(iso)), 1000);
    return () => clearInterval(timer);
  }, [iso]);
  return left;
}

/** Seconds as a clock, `m:ss`. */
export function mmss(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
