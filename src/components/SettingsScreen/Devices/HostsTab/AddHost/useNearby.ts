import { useEffect, useState } from "react";
import { api, type NearbyHost } from "@/api";

/** One browse, then a pause, round and round while the Hosts tab is mounted:
 *  a host that appears or leaves shows up within a few seconds, and nothing
 *  browses once the tab is gone. The phone's Pair screen does the same. */
const BROWSE_MS = 2000;
const PAUSE_MS = 2000;

/** The other Fletch hosts announcing themselves on this network.
 *  `searching` is true until the first browse has answered, so the list can
 *  tell "still looking" from "nobody here". */
export function useNearby(): { hosts: NearbyHost[]; searching: boolean } {
  const [hosts, setHosts] = useState<NearbyHost[]>([]);
  const [searching, setSearching] = useState(true);

  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const round = async () => {
      // A failed browse (no multicast on this network) lists nobody rather
      // than an error: pasting a link is still there.
      const found = await api.remoteBrowseNearby(BROWSE_MS).catch(() => [] as NearbyHost[]);
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
  }, []);

  return { hosts, searching };
}
