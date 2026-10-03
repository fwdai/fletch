import { useEffect, useRef, useState } from "react";

/** Re-render on an interval — for live elapsed timers. */
export function useTick(ms = 1000, active = true) {
  const [, bump] = useState(0);
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => bump((n) => n + 1), ms);
    return () => clearInterval(id);
  }, [ms, active]);
}

/** Run `fn` now and every `ms` while `active` — for background refreshes.
 *  `fn` is called through a ref, so a caller needn't memoize it. */
export function usePoll(fn: () => void, ms: number, active = true) {
  const latest = useRef(fn);
  latest.current = fn;
  useEffect(() => {
    if (!active) return;
    latest.current();
    const id = setInterval(() => latest.current(), ms);
    return () => clearInterval(id);
  }, [ms, active]);
}

export const fmtElapsed = (sec: number) => {
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return m ? `${m}m ${String(s).padStart(2, "0")}s` : `${s}s`;
};

/** "just now" / "2m ago" / "3h ago" / "2d ago" for an epoch-millis `at`. */
export const fmtAgo = (at: number, now = Date.now()) => {
  const sec = Math.max(0, Math.floor((now - at) / 1000));
  if (sec < 60) return "just now";
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
};

/** Whole seconds from `startedAt` (epoch millis) to `now`; 0 when not started. */
export const elapsedSec = (startedAt: number | undefined, now: number) =>
  startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : 0;

/** Seconds since `startedAt` (epoch millis), ticking while `active`. */
export function useElapsed(startedAt: number | undefined, active: boolean) {
  useTick(1000, active);
  return elapsedSec(startedAt, Date.now());
}

/** The OS colour scheme, kept live. */
export function useSystemTheme(onChange: (t: "light" | "dark") => void) {
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const apply = (matches: boolean) => onChange(matches ? "light" : "dark");
    apply(mq.matches);
    const handler = (e: MediaQueryListEvent) => apply(e.matches);
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [onChange]);
}
