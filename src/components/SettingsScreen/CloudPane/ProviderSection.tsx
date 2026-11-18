import { useState } from "react";
import type { CloudProviderStatus } from "@/api";
import { SetGroup } from "../primitives";
import { Connected } from "./Connected";
import { providerUi } from "./providers";
import { StatusLine } from "./StatusLine";

/** One provider, under its own name: connected, or the form that connects
 *  it. Once it is disconnected here, a line says what that left behind. */
export function ProviderSection({
  provider,
  last,
}: {
  provider: CloudProviderStatus;
  last?: boolean;
}) {
  const [disconnected, setDisconnected] = useState(false);
  const Form = providerUi(provider.id)?.Form;

  return (
    <SetGroup label={provider.label} last={last}>
      <div className="set-gh">
        {provider.connected ? (
          <Connected provider={provider} onDisconnected={() => setDisconnected(true)} />
        ) : Form ? (
          <Form provider={provider} />
        ) : (
          <StatusLine status="Not connected" />
        )}
        {disconnected && !provider.connected && (
          <div className="set-gh-hint text-sm">
            Machines already created at {provider.label} are not deleted by disconnecting.
          </div>
        )}
      </div>
    </SetGroup>
  );
}
