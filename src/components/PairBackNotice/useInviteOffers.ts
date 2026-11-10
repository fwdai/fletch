import { useCallback, useEffect, useState } from "react";
import { onInviteOffered } from "@/api";
import { pairWithOfferedLink } from "@/remote/hosts";
import { useAppStore } from "@/store";

/** Pair with each link a paired desktop offers (`remote:invite-offered`), and
 *  hold the name of the last one now controlled from here, for the notice. A
 *  failure goes to the app's error banner: the person here accepted that
 *  device moments ago, so there is nothing to ask, only to report. */
export function useInviteOffers(): { controlling: string | null; dismiss: () => void } {
  const [controlling, setControlling] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    const off = onInviteOffered((offer) => {
      pairWithOfferedLink(offer.url).then(
        () => {
          if (live) setControlling(offer.deviceName);
        },
        (e: unknown) => {
          const reason = e instanceof Error ? e.message : String(e);
          useAppStore
            .getState()
            .setLastError(`Couldn't connect to “${offer.deviceName}”: ${reason}`);
        },
      );
    });
    return () => {
      live = false;
      void off.then((f) => f());
    };
  }, []);

  const dismiss = useCallback(() => setControlling(null), []);
  return { controlling, dismiss };
}
