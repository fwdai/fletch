import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useRef, useState } from "react";
import { api } from "@/api";
import { useAppStore } from "@/store";

/** Where the automated `claude setup-token` flow is:
 *  - `idle`         — not started
 *  - `connecting`   — CLI launched; waiting on the consent URL / sign-in
 *  - `awaiting-code`— CLI is prompting for the auth code; show the input
 *  - `verifying`    — code submitted; waiting for the token to be captured
 *  - `success`      — token captured + stored
 *  - `error`        — the flow failed (see `error`) */
export type ClaudeSetupPhase =
  | "idle"
  | "connecting"
  | "awaiting-code"
  | "verifying"
  | "success"
  | "error";

/** Drives the automated `claude setup-token` capture end to end (the analogue
 *  of `useGithubConnect` for container auth): start the backend PTY flow, relay
 *  its consent URL and auth-code prompt to the UI, submit the code the user
 *  pastes back, then refresh the container-auth status on success.
 *
 *  A monotonic run id drops late results from a superseded/cancelled attempt:
 *  the backend PTY keeps running until it exits or is cancelled, but its outcome
 *  no longer touches the UI once a newer attempt (or a cancel) has started. */
export function useClaudeSetup(onConnected?: () => void) {
  const refreshContainerAuth = useAppStore((s) => s.refreshContainerAuth);

  const [phase, setPhase] = useState<ClaudeSetupPhase>("idle");
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const runRef = useRef(0);
  const onConnectedRef = useRef(onConnected);
  onConnectedRef.current = onConnected;

  const connect = useCallback(async () => {
    const runId = ++runRef.current;
    const stale = () => runRef.current !== runId;
    setPhase("connecting");
    setUrl(null);
    setError(null);
    // Default no-ops so `finally` can always call them; listen() lives inside
    // the try so an IPC failure surfaces as an error instead of throwing.
    let unlistenUrl: UnlistenFn = () => {};
    let unlistenCode: UnlistenFn = () => {};
    try {
      // Surface the consent URL as a fallback — the CLI already opens the
      // browser itself, so we show (not auto-open) it to avoid a duplicate tab.
      unlistenUrl = await listen<string>("claude-setup:url", (e) => {
        if (stale()) return;
        setUrl(e.payload);
      });
      unlistenCode = await listen("claude-setup:awaiting-code", () => {
        if (stale()) return;
        setPhase("awaiting-code");
      });
      await api.connectClaudeContainerAuth();
      if (stale()) return;
      await refreshContainerAuth();
      if (stale()) return;
      setPhase("success");
      onConnectedRef.current?.();
    } catch (err) {
      if (stale()) return;
      setError(String(err));
      setPhase("error");
    } finally {
      unlistenUrl();
      unlistenCode();
    }
  }, [refreshContainerAuth]);

  /** Send the auth code the user pasted to the waiting CLI; flips back to a
   *  verifying state until the connect call resolves (success) or rejects. */
  const submit = useCallback(async (code: string) => {
    try {
      await api.submitClaudeSetupCode(code);
      setPhase("verifying");
    } catch (err) {
      setError(String(err));
      setPhase("error");
    }
  }, []);

  /** Abandon the current attempt: invalidate its run id, tell the backend to
   *  kill the PTY, and reset to idle. Its now-superseded outcome is ignored. */
  const cancel = useCallback(() => {
    runRef.current++;
    void api.cancelClaudeContainerAuth().catch(() => {});
    setPhase("idle");
    setUrl(null);
    setError(null);
  }, []);

  return { phase, url, error, connect, submit, cancel };
}
