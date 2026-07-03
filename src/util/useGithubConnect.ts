import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useCallback, useRef, useState } from "react";
import { getOrCreateAccount, linkOAuthAccount, type OAuthProfile } from "@/storage/accounts";
import { useAppStore } from "@/store";

/** Info shown while a device-flow sign-in is pending: the user code to enter
 *  at the provider's verification page (opened automatically in the browser). */
export interface DeviceInfo {
  provider: string;
  userCode: string;
  verificationUri: string;
}

/** Drives an OAuth device-flow sign-in end to end, shared by onboarding, the
 *  New Project GitHub gate, and Settings. Owns the mechanics — emit the code,
 *  open the browser, poll, then persist the profile and refresh the account +
 *  GitHub connection — leaving each caller to render `device`/`error`/`busy`
 *  however it likes and pass an `onConnected` for its own follow-up (e.g. an
 *  onboarding step advance).
 *
 *  Late results from a superseded/cancelled attempt are dropped via a
 *  monotonic run id: the backend keeps polling until the code expires, but its
 *  outcome no longer touches the UI or the store. */
export function useGithubConnect(onConnected?: () => void) {
  const refreshAccount = useAppStore((s) => s.refreshAccount);
  const refreshGithub = useAppStore((s) => s.refreshGithub);

  const [device, setDevice] = useState<DeviceInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const runRef = useRef(0);
  const onConnectedRef = useRef(onConnected);
  onConnectedRef.current = onConnected;

  const connect = useCallback(
    async (provider = "github") => {
      if (busy) return;
      const runId = ++runRef.current;
      const stale = () => runRef.current !== runId;
      setBusy(provider);
      setError(null);
      setDevice(null);
      // Default no-op so `finally` can always call it; listen() lives inside
      // the try so an IPC failure surfaces as an error instead of throwing.
      let unlisten: UnlistenFn = () => {};
      try {
        unlisten = await listen<{
          provider: string;
          user_code: string;
          verification_uri: string;
        }>("oauth:device-code", (e) => {
          if (stale()) return;
          setDevice({
            provider: e.payload.provider,
            userCode: e.payload.user_code,
            verificationUri: e.payload.verification_uri,
          });
          void openExternal(e.payload.verification_uri).catch(() => {});
        });
        const profile = await invoke<OAuthProfile>("oauth_device_login", { provider });
        if (stale()) return;
        const account = await getOrCreateAccount();
        await linkOAuthAccount(account.id, profile);
        await refreshAccount();
        // GitHub sign-in also granted repo access + persisted a token; reflect
        // the connection so gated affordances unlock immediately.
        if (provider === "github") await refreshGithub();
        if (stale()) return;
        setBusy(null);
        setDevice(null);
        onConnectedRef.current?.();
      } catch (err) {
        if (stale()) return;
        setError(String(err));
        setBusy(null);
      } finally {
        unlisten();
      }
    },
    [busy, refreshAccount, refreshGithub],
  );

  /** Abandon the current attempt: invalidate its run id and clear UI state.
   *  The backend poll runs on until the code expires but is ignored. */
  const cancel = useCallback(() => {
    runRef.current++;
    setDevice(null);
    setError(null);
    setBusy(null);
  }, []);

  return { connect, cancel, device, error, busy };
}
