import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { CopyButton } from "@/components/ui/CopyButton";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { useClaudeSetup } from "@/util/useClaudeSetup";
import { SetRow } from "./primitives";

/** Human labels for each container-auth chain step (see the backend chain in
 *  `sandbox/docker/auth.rs` — first hit wins, so exactly one is active). */
const STATUS_LABELS: Record<string, string> = {
  "stored-token": "Using stored token",
  "shell-env": "Using API key from shell profile",
  "credentials-file": "Using claude credentials file",
  none: "Not connected",
};

const SETUP_COMMAND = "claude setup-token";

/** Settings › General › Sandbox: how containerized agents authenticate to
 *  Anthropic. Shows which chain step is active and opens the connect modal —
 *  the path for Keychain-only claude logins, which containers can't see.
 *  Docker-only: seatbelt agents keep the user's own login. */
export function ContainerAuth() {
  const containerAuth = useAppStore((s) => s.containerAuth);
  const refreshContainerAuth = useAppStore((s) => s.refreshContainerAuth);
  const [modalOpen, setModalOpen] = useState(false);

  // Resolve on mount so the row isn't stale; cheap after the first call (the
  // login-shell env is cached process-wide backend-side).
  useEffect(() => {
    void refreshContainerAuth();
  }, [refreshContainerAuth]);

  const status = containerAuth?.status;
  const connected = !!status && status !== "none";

  return (
    <>
      <SetRow
        title="Claude auth for containers"
        sub="Docker agents can't see your Keychain login, so they need an API key from your shell profile or a setup-token. Seatbelt agents keep using your own login."
      >
        <span className={`set-cauth-status text-sm ${connected ? "connected" : ""}`}>
          {status ? STATUS_LABELS[status] : "Checking…"}
        </span>
        <Button
          variant={status === "none" ? "primary" : "outline"}
          onClick={() => setModalOpen(true)}
        >
          {status === "none" ? "Connect Claude" : "Manage"}
        </Button>
      </SetRow>
      {modalOpen && <ContainerAuthModal onClose={() => setModalOpen(false)} />}
    </>
  );
}

/** "Connect Claude for containers" modal. The primary path is automated: it
 *  runs `claude setup-token` for the user (backend PTY flow via `useClaudeSetup`
 *  — surfaces the consent URL, collects the auth code, captures + stores the
 *  token). A manual paste field remains as a fallback (e.g. the `claude` CLI
 *  isn't on PATH). Reuses the GitHub connect modal's chrome (`ghc-*`). */
function ContainerAuthModal({ onClose }: { onClose: () => void }) {
  const status = useAppStore((s) => s.containerAuth?.status);
  const clearContainerAuthToken = useAppStore((s) => s.clearContainerAuthToken);
  // On success the hook refreshes the status row, then closes the modal.
  const { phase, url, error, connect, submit, cancel } = useClaudeSetup(onClose);
  const [manual, setManual] = useState(false);
  const [busy, setBusy] = useState(false);

  // Closing mid-flow cancels the backend PTY (no-op when idle).
  const close = () => {
    cancel();
    onClose();
  };

  const clear = async () => {
    setBusy(true);
    await clearContainerAuthToken();
    setBusy(false);
    onClose();
  };

  return (
    <>
      <Scrim onClose={close} zIndex={400} />
      <div className="ghc-modal" role="dialog" aria-modal="true">
        <div className="ghc-h flex-center text-base">
          <Icon name="cube" size={15} />
          <span>Connect Claude for containers</span>
          <button className="ghc-close flex-center" aria-label="Close" onClick={close}>
            <Icon name="close" size={14} />
          </button>
        </div>

        <div className="ghc-body">
          {manual ? (
            <ManualPaste onClose={onClose} onBack={() => setManual(false)} />
          ) : (
            <AutoConnect phase={phase} url={url} error={error} connect={connect} submit={submit} />
          )}

          <div className="ghc-actions flex-center">
            {!manual && phase === "idle" && (
              <Button variant="ghost" onClick={() => setManual(true)}>
                Paste a token manually
              </Button>
            )}
            {status === "stored-token" && (
              <Button variant="outline" disabled={busy} onClick={() => void clear()}>
                Clear token
              </Button>
            )}
            <Button variant="outline" onClick={close}>
              {phase === "success" ? "Done" : "Cancel"}
            </Button>
          </div>
        </div>
      </div>
    </>
  );
}

/** The automated flow, rendered by phase. */
function AutoConnect({
  phase,
  url,
  error,
  connect,
  submit,
}: {
  phase: ReturnType<typeof useClaudeSetup>["phase"];
  url: string | null;
  error: string | null;
  connect: () => void;
  submit: (code: string) => void;
}) {
  const [code, setCode] = useState("");

  const urlRow = url && (
    <div className="set-cauth-cmd flex-center">
      <span className="set-cauth-url mono text-sm" title={url}>
        {url}
      </span>
      <CopyButton text={url} />
      <Button variant="outline" onClick={() => void openExternal(url).catch(() => {})}>
        Open
      </Button>
    </div>
  );

  if (phase === "idle") {
    return (
      <>
        <div className="ghc-lede text-sm">
          Connect your Claude account so containerized agents can authenticate. This opens your
          browser once to sign in — no copy-paste.
        </div>
        <div className="ghc-actions flex-center">
          <Button variant="primary" onClick={connect}>
            Connect Claude
          </Button>
        </div>
      </>
    );
  }

  if (phase === "awaiting-code") {
    return (
      <>
        <div className="ghc-lede text-sm">
          Finish signing in, then paste the code from your browser here.
        </div>
        {urlRow}
        <input
          className="set-cauth-input mono text-sm"
          type="text"
          placeholder="Paste code here"
          value={code}
          autoFocus
          onChange={(e) => setCode(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && code.trim()) submit(code);
          }}
        />
        <div className="ghc-actions flex-center">
          <Button variant="primary" disabled={!code.trim()} onClick={() => submit(code)}>
            Submit code
          </Button>
        </div>
      </>
    );
  }

  if (phase === "success") {
    return (
      <div className="ghc-lede flex-center text-sm">
        <Icon name="check" size={14} />
        <span>Connected. Containers can now authenticate to Claude.</span>
      </div>
    );
  }

  if (phase === "error") {
    return (
      <>
        <div className="ghc-err text-sm">{error ?? "Something went wrong."}</div>
        <div className="ghc-actions flex-center">
          <Button variant="primary" onClick={connect}>
            Try again
          </Button>
        </div>
      </>
    );
  }

  // connecting | verifying
  return (
    <>
      <div className="set-cauth-note text-sm">
        {phase === "verifying" ? "Verifying your code…" : "Opening your browser to sign in…"}
      </div>
      {urlRow}
    </>
  );
}

/** Fallback: run `claude setup-token` yourself and paste the result. */
function ManualPaste({ onClose, onBack }: { onClose: () => void; onBack: () => void }) {
  const setContainerAuthToken = useAppStore((s) => s.setContainerAuthToken);
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await setContainerAuthToken(token);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="ghc-lede text-sm">
        Run <span className="mono">{SETUP_COMMAND}</span> in your terminal and paste the token here.
      </div>
      <div className="set-cauth-cmd flex-center">
        <span className="mono text-sm">{SETUP_COMMAND}</span>
        <CopyButton text={SETUP_COMMAND} />
      </div>
      <input
        className="set-cauth-input mono text-sm"
        type="password"
        placeholder="sk-ant-oat…"
        value={token}
        autoFocus
        onChange={(e) => {
          setToken(e.target.value);
          setError(null);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && token.trim() && !busy) void save();
        }}
      />
      {error && <div className="ghc-err text-sm">{error}</div>}
      <div className="ghc-actions flex-center">
        <Button variant="primary" disabled={busy || !token.trim()} onClick={() => void save()}>
          Save token
        </Button>
        <Button variant="ghost" onClick={onBack}>
          Back
        </Button>
      </div>
    </>
  );
}
