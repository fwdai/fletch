import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { CopyButton } from "@/components/ui/CopyButton";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { SetRow } from "./primitives";

/** Human labels for each container-auth chain step (see the backend chain in
 *  `sandbox/docker/auth.rs` — first hit wins, so exactly one is active). */
const STATUS_LABELS: Record<string, string> = {
  "stored-token": "Using pasted token",
  "shell-env": "Using API key from environment",
  "credentials-file": "Using claude credentials file",
  none: "Not connected",
};

const SETUP_COMMAND = "claude setup-token";

/** Settings › General › Sandbox: how containerized agents authenticate to
 *  Anthropic. Shows which chain step is active and opens the setup-token
 *  modal — the path for Keychain-only claude logins, which containers can't
 *  see. Docker-only: seatbelt agents keep the user's own login. */
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
        sub="Docker agents can't see your Keychain login, so they need an API key from your environment (shell profile or the launching terminal) or a pasted setup-token. Seatbelt agents keep using your own login."
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

/** "Connect Claude for containers" modal: copyable `claude setup-token`
 *  command + paste field. Reuses the GitHub connect modal's chrome (`ghc-*`,
 *  see components/GithubConnect) — same compact centered dialog. */
function ContainerAuthModal({ onClose }: { onClose: () => void }) {
  const status = useAppStore((s) => s.containerAuth?.status);
  const setContainerAuthToken = useAppStore((s) => s.setContainerAuthToken);
  const clearContainerAuthToken = useAppStore((s) => s.clearContainerAuthToken);
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

  const clear = async () => {
    setBusy(true);
    await clearContainerAuthToken();
    setBusy(false);
    onClose();
  };

  return (
    <>
      <Scrim onClose={onClose} zIndex={400} />
      <div className="ghc-modal" role="dialog" aria-modal="true">
        <div className="ghc-h flex-center text-base">
          <Icon name="cube" size={15} />
          <span>Connect Claude for containers</span>
          <button className="ghc-close flex-center" aria-label="Close" onClick={onClose}>
            <Icon name="close" size={14} />
          </button>
        </div>

        <div className="ghc-body">
          <div className="ghc-lede text-sm">
            Run <span className="mono">{SETUP_COMMAND}</span> in your terminal and paste the token
            here.
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
            {status === "stored-token" && (
              <Button variant="outline" disabled={busy} onClick={() => void clear()}>
                Clear token
              </Button>
            )}
            <Button variant="outline" onClick={onClose}>
              Cancel
            </Button>
          </div>
        </div>
      </div>
    </>
  );
}
