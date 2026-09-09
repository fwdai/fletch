import { useEffect, useState } from "react";

/** Seconds since `startedAt`, ticking while it is set; 0 when it is null. The
 *  tick lives here so the clock re-renders alone, not the composer around it. */
export function useElapsed(startedAt: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (startedAt === null) return;
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(id);
  }, [startedAt]);
  return startedAt === null ? 0 : Math.max(0, (now - startedAt) / 1000);
}
