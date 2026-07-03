import { useEffect } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { useGithubConnect } from "@/util/useGithubConnect";

/** GitHub connection control for Settings → Account. Connecting runs the
 *  device flow inline (the same one onboarding uses); disconnecting drops the
 *  stored token and returns the app to local-only mode. */
export function GithubConnection() {
  const github = useAppStore((s) => s.github);
  const refreshGithub = useAppStore((s) => s.refreshGithub);
  const disconnectGithub = useAppStore((s) => s.disconnectGithub);
  const { connect, cancel, device, error, busy } = useGithubConnect();

  // Reflect the current connection on mount (and after a connect/disconnect
  // elsewhere) so the row isn't stale.
  useEffect(() => {
    void refreshGithub();
  }, [refreshGithub]);

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
          <Button variant="outline" onClick={() => void disconnectGithub()}>
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
    </div>
  );
}
