import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useCallback, useRef, useState } from "react";
import { localTransport, type UnlistenFn } from "@/api/transport";
import { getOrCreateAccount, linkOAuthAccount, type OAuthProfile } from "@/storage/accounts";
import { useAppStore } from "@/store";
import { connectFailureReason } from "./connectFailure";
import { type ConnectSource, track } from "./track";

/** Info shown while a device-flow sign-in is pending: the user code to enter
 *  at the provider's verification page (opened automatically in the browser). */
export interface DeviceInfo {
  provider: string;
  userCode: string;
  verificationUri: string;
}

const PROVIDER = "github";

/** Drives the GitHub OAuth device-flow sign-in end to end, shared by the
 *  onboarding GitHub step, the New Project GitHub gate, the app-level connect
 *  modal and Settings. Owns the mechanics — emit the code, open the browser,
 *  poll, then persist the profile and refresh the account + GitHub connection
 *  — leaving each caller to render `device`/`error`/`busy` however it likes
 *  and pass an `onConnected` for its own follow-up (e.g. re-running the
 *  onboarding checks).
 *
 *  Late results from a superseded/cancelled attempt are dropped via a
 *  monotonic run id: the backend keeps polling until the code expires, but its
 *  outcome no longer touches the UI or the store.
 *
 *  Telemetry lives here rather than at the call sites so every entry point is
 *  measured identically; `source` is the only thing a caller supplies, and it
 *  separates an onboarding sign-in (funnel drop-off) from one the user hit
 *  mid-task. The device flow sends the user to a browser to paste a code —
 *  the likeliest place to strand someone — so started/succeeded/failed/
 *  cancelled are all reported, and a failure carries a category saying why. */
export function useGithubConnect(onConnected?: () => void, source: ConnectSource = "settings") {
  const refreshAccount = useAppStore((s) => s.refreshAccount);
  const refreshGithub = useAppStore((s) => s.refreshGithub);

  const [device, setDevice] = useState<DeviceInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const runRef = useRef(0);
  const onConnectedRef = useRef(onConnected);
  onConnectedRef.current = onConnected;

  const connect = useCallback(async () => {
    if (busy) return;
    const runId = ++runRef.current;
    const stale = () => runRef.current !== runId;
    setBusy(true);
    setError(null);
    setDevice(null);
    track("github_connect_started", { source, provider: PROVIDER });
    // Default no-op so `finally` can always call it; the subscribe lives
    // inside the try so an IPC failure surfaces as an error instead of
    // throwing.
    let unlisten: UnlistenFn = () => {};
    try {
      unlisten = await localTransport.on<{
        provider: string;
        user_code: string;
        verification_uri: string;
      }>("oauth:device-code", (code) => {
        if (stale()) return;
        setDevice({
          provider: code.provider,
          userCode: code.user_code,
          verificationUri: code.verification_uri,
        });
        void openExternal(code.verification_uri).catch(() => {});
      });
      const profile = await localTransport.call<OAuthProfile>("oauth_device_login", {
        provider: PROVIDER,
      });
      if (stale()) return;
      const account = await getOrCreateAccount();
      await linkOAuthAccount(account.id, profile);
      await refreshAccount();
      // The sign-in also granted repo access + persisted a token; reflect the
      // connection so gated affordances unlock immediately.
      await refreshGithub();
      if (stale()) return;
      setBusy(false);
      setDevice(null);
      track("github_connect_succeeded", { source, provider: PROVIDER });
      onConnectedRef.current?.();
    } catch (err) {
      if (stale()) return;
      const message = String(err);
      setError(message);
      setBusy(false);
      // Categorical only — the raw error can carry a URL or account handle.
      track("github_connect_failed", {
        source,
        provider: PROVIDER,
        reason: connectFailureReason(message),
      });
    } finally {
      unlisten();
    }
  }, [busy, refreshAccount, refreshGithub, source]);

  /** Abandon the current attempt: invalidate its run id and clear UI state.
   *  The backend poll runs on until the code expires but is ignored. */
  const cancel = useCallback(() => {
    runRef.current++;
    // Guarded on `busy` so only a live attempt counts as abandoned: the error
    // branch clears `busy` first, and its Cancel button is a dismissal (already
    // reported as `github_connect_failed`), not a second outcome.
    if (busy) track("github_connect_cancelled", { source, provider: PROVIDER });
    setDevice(null);
    setError(null);
    setBusy(false);
  }, [busy, source]);

  return { connect, cancel, device, error, busy };
}
