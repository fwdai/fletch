import { useState } from "react";
import type { CloudProviderStatus } from "@/api";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { StatusLine } from "./StatusLine";

/** Where a connected provider stands, in words: the account its token acts
 *  for, under the provider's own name for one. */
function connectedLine({ account, account_label }: CloudProviderStatus): string {
  if (!account) return "Connected";
  const noun = account_label.trim().toLowerCase();
  return noun ? `Connected to the ${account} ${noun}` : `Connected as ${account}`;
}

/** A connected provider: the account its token acts for, and Disconnect,
 *  which deletes the provider's spare machines and then forgets the token.
 *  The engine refuses it while a spare cannot be deleted or this install's
 *  runner is at the provider; its words show under the row, which stays
 *  connected. */
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
