import { useCallback, useEffect, useState } from "react";
import { api, type GhStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { useGithubConnect } from "@/util/useGithubConnect";
import { HostAccountNote } from "./HostAccountNote";

/** GitHub connection control for Settings → Account. Connecting runs the
 *  device flow inline (the same one onboarding uses); disconnecting drops the
 *  stored token and returns the app to local-only mode.
 *
 *  Both act on THIS Mac — `oauth_device_login` is local by construction — so
 *  the status beside them is read locally too, rather than from the store's
 *  `github`, which follows the active environment. A host is signed in on the
 *  host; [`HostAccountNote`] says so. */
export function GithubConnection() {
  const [github, setGithub] = useState<GhStatus | null>(null);
  const disconnectGithub = useAppStore((s) => s.disconnectGithub);

  // Reflect the current connection on mount (and after a connect/disconnect
  // elsewhere) so the row isn't stale.
  const probe = useCallback(() => {
    void api
      .ghStatus()
      .then(setGithub)
      .catch(() => setGithub({ installed: true, authenticated: false, login: null }));
  }, []);
  const { connect, cancel, device, error, busy } = useGithubConnect(probe);

  useEffect(probe, [probe]);

  if (device) {
    return (
      <div className="set-gh">
        <div className="set-gh-lede text-sm">Finish signing in in your browser, then enter:</div>
        <div className="set-gh-code text-2xl">{device.userCode}</div>
        <div className="set-gh-uri mono text-sm">{device.verificationUri}</div>
        <Button variant="outline" onClick={cancel}>
          Cancel
        </Button>
      </div>
    );
  }

  const connected = !!github?.authenticated;

  return (
    <div className="set-gh">
      <div className="set-gh-row flex-center">
        <Icon name="github" size={16} />
        <span className="set-gh-status text-base">
          {connected
            ? `Connected${github?.login ? ` as ${github.login}` : ""}`
            : "Not connected — local projects still work offline"}
        </span>
        {connected ? (
          <Button variant="outline" onClick={() => void disconnectGithub().then(probe)}>
            Disconnect
          </Button>
        ) : (
          <Button variant="primary" disabled={!!busy} onClick={() => void connect()}>
            {busy ? "Connecting…" : "Connect GitHub"}
          </Button>
        )}
      </div>
      {error && <div className="set-gh-err text-sm">{error}</div>}
      {connected && (
        <div className="set-gh-hint text-sm">
          Fletch uses this to clone, push, and open pull requests on your behalf.
        </div>
      )}
      <HostAccountNote />
    </div>
  );
}
