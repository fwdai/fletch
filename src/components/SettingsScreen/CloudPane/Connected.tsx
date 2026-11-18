import { useState } from "react";
import type { CloudProviderStatus } from "@/api";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { connectedLine } from "./providers";
import { StatusLine } from "./StatusLine";

/** A connected provider: the account its token acts for, and Disconnect,
 *  which forgets the token at once, as the Account pane's GitHub and Linear
 *  rows do. */
export function Connected({
  provider,
  onDisconnected,
}: {
  provider: CloudProviderStatus;
  onDisconnected: () => void;
}) {
  const disconnectCloud = useAppStore((s) => s.disconnectCloud);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const disconnect = async () => {
    setBusy(true);
    setError(null);
    try {
      await disconnectCloud(provider.id);
      onDisconnected();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <StatusLine status={connectedLine(provider)}>
        <Button variant="outline" disabled={busy} onClick={() => void disconnect()}>
          Disconnect
        </Button>
      </StatusLine>
      {error && <div className="set-gh-err text-sm">{error}</div>}
    </>
  );
}
