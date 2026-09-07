import { useEffect, useState } from "react";

/** Re-render on an interval — for live elapsed timers. */
export function useTick(ms = 1000, active = true) {
  const [, bump] = useState(0);
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => bump((n) => n + 1), ms);
    return () => clearInterval(id);
  }, [ms, active]);
}

export const fmtElapsed = (sec: number) => {
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return m ? `${m}m ${String(s).padStart(2, "0")}s` : `${s}s`;
};

/** Seconds since `startedAt` (epoch millis), ticking while `active`. */
export function useElapsed(startedAt: number | undefined, active: boolean) {
  useTick(1000, active);
  if (!startedAt) return 0;
  return Math.max(0, Math.floor((Date.now() - startedAt) / 1000));
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
