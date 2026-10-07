import { useEffect, useState } from "react";

/** The current time, re-read every `intervalMs` — enough for a reset
 *  countdown in minutes and an "as of" age to stay true while Settings sits
 *  open. */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);
  return now;
}
