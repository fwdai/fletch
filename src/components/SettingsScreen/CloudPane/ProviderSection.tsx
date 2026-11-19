import { useState } from "react";
import type { CloudProviderStatus } from "@/api";
import { SetGroup } from "../primitives";
import { Connected } from "./Connected";
import { ConnectForm } from "./ConnectForm";
import { Runner } from "./Runner";

/** One provider, under its own name: connected, or the form that connects
 *  it. Once it is disconnected here, a line says what that did and did not
 *  delete: the engine deletes the provider's spare machines before it
 *  disconnects, and never a workspace's. A connected provider shows this
 *  install's runner too (`Runner`). */
export function ProviderSection({
  provider,
  runnerError,
  last,
}: {
  provider: CloudProviderStatus;
  /** Why the runner's status could not be read, when it never was. */
  runnerError: string | null;
  last?: boolean;
}) {
  const [disconnected, setDisconnected] = useState(false);

  return (
    <SetGroup label={provider.label} last={last}>
      <div className="set-gh">
        {provider.connected ? (
          <Connected provider={provider} onDisconnected={() => setDisconnected(true)} />
        ) : (
          <ConnectForm provider={provider} />
        )}
        {disconnected && !provider.connected && (
          <div className="set-gh-hint text-sm">
            Disconnecting deleted any spare machines at {provider.label}; the machines of workspaces
            there stay.
          </div>
        )}
      </div>
      {provider.connected && <Runner provider={provider} readError={runnerError} />}
    </SetGroup>
  );
}
