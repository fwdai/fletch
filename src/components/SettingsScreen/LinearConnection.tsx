import { useEffect, useState } from "react";
import { api } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";

/** Linear connection control for Settings › Account: the app-wide personal API
 *  key, stored in the keychain and shared by every project. Which team a
 *  project draws tickets from stays on that project's screen. Same chrome as
 *  `GithubConnection` (`set-gh-*`). */
export function LinearConnection() {
  const linear = useAppStore((s) => s.linear);
  const refreshLinear = useAppStore((s) => s.refreshLinear);
  const [apiKey, setApiKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void refreshLinear();
  }, [refreshLinear]);

  const connected = !!linear?.authenticated;

  const connect = async () => {
    const key = apiKey.trim();
    if (!key || busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.linearConnect(key);
      setApiKey("");
      await refreshLinear();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async () => {
    setError(null);
    try {
      await api.linearDisconnect();
      await refreshLinear();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="set-gh">
      <div className="set-gh-row flex-center">
        <Icon name="issue" size={16} />
        <span className="set-gh-status text-base">
          {connected
            ? `Connected${linear?.user ? ` as ${linear.user}` : ""}`
            : "Not connected — pick a team per project once connected"}
        </span>
        {connected ? (
          <Button variant="outline" onClick={() => void disconnect()}>
            Disconnect
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={!apiKey.trim() || busy}
            onClick={() => void connect()}
          >
            {busy ? "Connecting…" : "Connect Linear"}
          </Button>
        )}
      </div>
      {!connected && (
        <>
          <TextInput
            mono
            type="password"
            placeholder="lin_api_…"
            value={apiKey}
            spellCheck={false}
            autoComplete="off"
            aria-label="Linear personal API key"
            onChange={(e) => {
              setApiKey(e.target.value);
              if (error) setError(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") void connect();
            }}
          />
          <div className="set-gh-hint text-sm">
            Create a personal API key in Linear under Settings → Security & access → API keys.
          </div>
        </>
      )}
      {error && <div className="set-gh-err text-sm">{error}</div>}
      {connected && (
        <div className="set-gh-hint text-sm">
          Linear tickets join GitHub issues on Home and in the composer for projects with a team
          set.
        </div>
      )}
    </div>
  );
}
