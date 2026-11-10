import { useCallback, useEffect, useState } from "react";
import { addHost, removeHost } from "@/remote/hosts";
import type { PairTarget } from "@/remote/registry";
import type { CodeHandler } from "@/remote/types";
import { loadHosts, type SavedHost } from "@/storage/remoteHosts";

/** Pair with a host and keep it. Rejects with the reason, already in words
 *  the pane can show. `onCode` hears a confirmed pairing's six digits. */
export type AddHost = (target: PairTarget, onCode?: CodeHandler) => Promise<void>;

export interface PairedHosts {
  hosts: SavedHost[];
  add: AddHost;
  forget: (hostKey: string) => Promise<void>;
  /** A forget is in flight. */
  busy: boolean;
}

/** The Hosts tab's saved records, and adding to and removing from them.
 *  Local rather than a store slice, like `useRemote` beside it — the live
 *  connections are in the store, and this is only the pane's view of the
 *  records behind them. Each way of adding keeps its own field and error. */
export function usePairedHosts(): PairedHosts {
  const [hosts, setHosts] = useState<SavedHost[]>([]);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => setHosts(await loadHosts()), []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const add = useCallback<AddHost>(
    async (target, onCode) => {
      await addHost(target, onCode);
      await reload();
    },
    [reload],
  );

  const forget = useCallback(
    async (hostKey: string) => {
      setBusy(true);
      try {
        await removeHost(hostKey);
        await reload();
      } finally {
        setBusy(false);
      }
    },
    [reload],
  );

  return { hosts, add, forget, busy };
}
