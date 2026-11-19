import { useEffect, useState } from "react";
import { useAppStore } from "@/store";
import { SetHead } from "../primitives";
import { ProviderSection } from "./ProviderSection";
import { useRunnerStatus } from "./Runner/useRunnerStatus";
import { SparesRow } from "./SparesRow";

/** Settings › Cloud: the providers whose machines a workspace can run on
 *  instead of this Mac, one section each, in the order the backend lists them,
 *  with this install's runner under the provider it is at; then, once one is
 *  connected, how many spare machines each keeps. All of it is this Mac's (the
 *  tokens are in its keychain), so it never follows a host picked elsewhere in
 *  Settings. The list and the runner are the store's (`cloudProviders`,
 *  `cloudRunner`), re-read on every visit; what is being typed into a form
 *  stays in that form. */
export function CloudPane() {
  const providers = useAppStore((s) => s.cloudProviders);
  const refresh = useAppStore((s) => s.refreshCloudProviders);
  const [error, setError] = useState<string | null>(null);
  const runnerError = useRunnerStatus();
  const connected = !!providers?.some((p) => p.connected);

  useEffect(() => {
    refresh().then(
      () => setError(null),
      (e) => setError(String(e)),
    );
  }, [refresh]);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Cloud"
        title="Cloud"
        desc="Workspaces can run on a machine at a cloud provider instead of on this Mac. The provider bills your account for its machines; your code is sent there, your GitHub login is not."
      />
      {error && <div className="set-inline-warn">Couldn't read the cloud providers: {error}</div>}
      {providers?.map((p, i) => (
        <ProviderSection
          key={p.id}
          provider={p}
          runnerError={runnerError}
          last={!connected && i === providers.length - 1}
        />
      ))}
      {connected && <SparesRow last />}
    </div>
  );
}
