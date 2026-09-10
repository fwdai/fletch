import { useCallback, useEffect, useState } from "react";
import { api, type PairingInvite, type RemoteStatus } from "@/api";

/** How often the pane re-reads status. A phone connecting or dropping produces
 *  no Tauri event (the remote server is the thing being observed, not an agent),
 *  so the connected dot is a poll. Only while Settings is open. */
const POLL_MS = 4_000;

export interface Remote {
  status: RemoteStatus | null;
  invite: PairingInvite | null;
  error: string | null;
  busy: boolean;
  setEnabled: (enabled: boolean) => Promise<void>;
  setPort: (port: number) => Promise<void>;
  setRelay: (url: string | null) => Promise<void>;
  beginPairing: () => Promise<void>;
  revoke: (deviceId: string) => Promise<void>;
  clearInvite: () => void;
}

/** Server state for the Remote control pane. Deliberately local rather than
 *  a store slice: nothing outside this pane reads it, and it is only live while
 *  the pane is mounted. */
export function useRemote(): Remote {
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [invite, setInvite] = useState<PairingInvite | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.remoteStatus());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  /** Run a mutating command; they all answer with the fresh status. */
  const mutate = useCallback(async (call: () => Promise<RemoteStatus>) => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await call());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const setEnabled = useCallback(
    async (enabled: boolean) => {
      // A code minted against a listener that is going away is unusable.
      if (!enabled) setInvite(null);
      await mutate(() => api.remoteSetEnabled(enabled));
    },
    [mutate],
  );

  const setPort = useCallback(
    async (port: number) => {
      // A pairing code names the old port, so it is stale the moment this lands.
      setInvite(null);
      await mutate(() => api.remoteSetPort(port));
    },
    [mutate],
  );

  const setRelay = useCallback(
    (url: string | null) => mutate(() => api.remoteSetRelay(url)),
    [mutate],
  );

  const beginPairing = useCallback(async () => {
    setError(null);
    try {
      setInvite(await api.remoteBeginPairing());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const revoke = useCallback(
    (deviceId: string) => mutate(() => api.remoteRevokeDevice(deviceId)),
    [mutate],
  );

  const clearInvite = useCallback(() => setInvite(null), []);

  return {
    status,
    invite,
    error,
    busy,
    setEnabled,
    setPort,
    setRelay,
    beginPairing,
    revoke,
    clearInvite,
  };
}
