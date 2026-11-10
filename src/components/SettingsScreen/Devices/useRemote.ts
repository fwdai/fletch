import { useCallback, useEffect, useState } from "react";
import {
  api,
  onPairingClosed,
  type PairingInvite,
  type PairingPreset,
  type RemoteStatus,
} from "@/api";

/** How often the pane re-reads status. A phone connecting or dropping produces
 *  no Tauri event (the remote server is the thing being observed, not an agent),
 *  so the connected dot is a poll. Only while Settings is open. */
const POLL_MS = 4_000;

export interface Remote {
  status: RemoteStatus | null;
  invite: PairingInvite | null;
  /** The host closed the invite's window before it expired (too many pairing
   *  requests from the network): its code no longer works. */
  inviteClosed: boolean;
  /** Why the last command failed, else why the last invite could not be
   *  minted. */
  error: string | null;
  busy: boolean;
  /** Turning remote access on brings Fletch's relay with it; the host sees to
   *  that (`remote_set_enabled`), so there is no relay setter here. */
  setEnabled: (enabled: boolean) => Promise<void>;
  setPort: (port: number) => Promise<void>;
  /** Mint a code granting `preset`. The host refuses a preset it does not
   *  define, which surfaces as `error`. */
  beginPairing: (preset: PairingPreset) => Promise<void>;
  revoke: (deviceId: string) => Promise<void>;
}

/** Server state for the Devices page. Deliberately local rather than a store
 *  slice: nothing outside this page reads it, and it is only live while the
 *  page is mounted. */
export function useRemote(): Remote {
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [invite, setInvite] = useState<PairingInvite | null>(null);
  const [inviteClosed, setInviteClosed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [inviteError, setInviteError] = useState<string | null>(null);
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

  // The host closes the window itself, and anyone on the network can make it:
  // the card must stop offering a code that no longer works.
  useEffect(() => {
    const off = onPairingClosed(() => setInviteClosed(true));
    return () => void off.then((f) => f());
  }, []);

  /** Run a mutating command; they all answer with the fresh status. */
  const mutate = useCallback(async (call: () => Promise<RemoteStatus>) => {
    setBusy(true);
    setError(null);
    setInviteError(null);
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

  // Its own error, not `error`: the page mints invites on its own, and one
  // landing right after a failed command must not wipe that command's reason.
  const beginPairing = useCallback(async (preset: PairingPreset) => {
    try {
      setInvite(await api.remoteBeginPairing(preset));
      setInviteClosed(false);
      setInviteError(null);
    } catch (e) {
      setInviteError(String(e));
    }
  }, []);

  const revoke = useCallback(
    (deviceId: string) => mutate(() => api.remoteRevokeDevice(deviceId)),
    [mutate],
  );

  return {
    status,
    invite,
    inviteClosed,
    error: error ?? inviteError,
    busy,
    setEnabled,
    setPort,
    beginPairing,
    revoke,
  };
}
