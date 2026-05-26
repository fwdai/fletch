import { useEffect, useState } from "react";

/** Re-render once a minute so age strings ("5m", "2h") stay fresh. */
export function useMinuteClock(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(id);
  }, []);
  return now;
}
