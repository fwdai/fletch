import { useCallback, useEffect, useState } from "react";
import { addHost, removeHost } from "@/remote/hosts";
import { loadHosts, type SavedHost } from "@/storage/remoteHosts";
import { pairTargetFrom } from "./pairLink";

export interface PairedHosts {
  hosts: SavedHost[];
  /** The pasted link, as typed. */
  link: string;
  setLink: (value: string) => void;
  /** Why the last add was refused — a link that isn't one, or a handshake the
   *  host turned down. Cleared as soon as the field is edited. */
  error: string | null;
  busy: boolean;
  add: () => Promise<void>;
  forget: (hostKey: string) => Promise<void>;
}

/** The "Paired hosts" section's own state: the saved records, the link being
 *  pasted, and whichever of the two failed last. Local rather than a store
 *  slice, like `useRemote` beside it — the live connections are in the store,
 *  and this is only the pane's view of adding to and removing from them. */
export function usePairedHosts(): PairedHosts {
  const [hosts, setHosts] = useState<SavedHost[]>([]);
  const [link, setLinkValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void loadHosts().then(setHosts);
  }, []);

  const setLink = useCallback((value: string) => {
    setLinkValue(value);
    setError(null);
  }, []);

  const add = useCallback(async () => {
    const parsed = pairTargetFrom(link);
    if ("error" in parsed) {
      setError(parsed.error);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await addHost(parsed.target);
      setLinkValue("");
      setHosts(await loadHosts());
    } catch (e) {
      // The client has already turned the host's or the transport's words into
      // something presentable.
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [link]);

  const forget = useCallback(async (hostKey: string) => {
    setBusy(true);
    try {
      await removeHost(hostKey);
      setHosts(await loadHosts());
    } finally {
      setBusy(false);
    }
  }, []);

  return { hosts, link, setLink, error, busy, add, forget };
}
