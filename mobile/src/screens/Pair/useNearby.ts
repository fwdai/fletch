import { useEffect, useState } from "react";
import { browseNearby, type NearbyHost } from "../../remote/nearby";

/** How long each browse listens, and the pause before the next. A Mac whose
 *  remote control is switched on while this screen is open shows up within a
 *  few seconds; nothing runs once the screen is gone. */
const BROWSE_MS = 2000;
const PAUSE_MS = 2000;

/** The Fletch hosts announcing themselves on this network, refreshed while
 *  `active`. `searching` is true until the first browse has answered, so the
 *  list can tell "still looking" from "nobody here". */
export function useNearby(active: boolean): { hosts: NearbyHost[]; searching: boolean } {
  const [hosts, setHosts] = useState<NearbyHost[]>([]);
  const [searching, setSearching] = useState(true);

  useEffect(() => {
    if (!active) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const round = async () => {
      const found = await browseNearby(BROWSE_MS).catch(() => [] as NearbyHost[]);
      if (stopped) return;
      setHosts(found);
      setSearching(false);
      timer = setTimeout(() => void round(), PAUSE_MS);
    };
    void round();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [active]);

  return { hosts, searching };
}
