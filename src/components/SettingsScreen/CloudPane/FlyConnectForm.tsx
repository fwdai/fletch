import { type KeyboardEvent, useState } from "react";
import type { CloudProviderStatus } from "@/api";
import { Button } from "@/components/ui/Button";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";
import { StatusLine } from "./StatusLine";

/** Connecting Fly.io: an org token, and the organization it is for when that
 *  is not the personal one (the backend's default for an empty field). The
 *  backend checks the token with Fly before keeping it, so a refusal shows
 *  here with the fields as typed. The token is held in this form alone until
 *  it is sent, and cleared once it is kept. */
export function FlyConnectForm({ provider }: { provider: CloudProviderStatus }) {
  const connectCloud = useAppStore((s) => s.connectCloud);
  const [token, setToken] = useState("");
  const [org, setOrg] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const connect = async () => {
    const key = token.trim();
    if (!key || busy) return;
    setBusy(true);
    setError(null);
    try {
      await connectCloud(provider.id, key, org.trim());
      setToken("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") void connect();
  };

  return (
    <>
      <StatusLine status="Not connected">
        <Button variant="primary" disabled={!token.trim() || busy} onClick={() => void connect()}>
          {busy ? "Connecting…" : `Connect ${provider.label}`}
        </Button>
      </StatusLine>
      <div className="set-form-grid">
        <label className="set-field">
          <span className="set-field-label text-sm">Token</span>
          <TextInput
            mono
            type="password"
            placeholder="FlyV1 fm2_…"
            value={token}
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => {
              setToken(e.target.value);
              if (error) setError(null);
            }}
            onKeyDown={onKeyDown}
          />
        </label>
        <label className="set-field">
          <span className="set-field-label text-sm">Organization</span>
          <TextInput
            mono
            placeholder="personal"
            value={org}
            disabled={busy}
            spellCheck={false}
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            onChange={(e) => {
              setOrg(e.target.value);
              if (error) setError(null);
            }}
            onKeyDown={onKeyDown}
          />
        </label>
      </div>
      <div className="set-gh-hint text-sm">
        Run <span className="mono">fly tokens create org</span> in a terminal, or create one in the
        Fly.io dashboard under Tokens. It is kept in this Mac's keychain.
      </div>
      {error && <div className="set-gh-err text-sm">{error}</div>}
    </>
  );
}
