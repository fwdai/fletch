import { useEffect, useState } from "react";
import { useAppStore } from "@/store";
import { SetHead } from "../primitives";
import { ProviderSection } from "./ProviderSection";

/** Settings › Cloud: the providers whose machines a workspace can run on
 *  instead of this Mac, one section each, in the order the backend lists them.
 *  All of it is this Mac's (the tokens are in its keychain), so it never
 *  follows a host picked elsewhere in Settings. The list is the store's
 *  (`cloudProviders`), re-read on every visit; what is being typed into a form
 *  stays in that form. */
export function CloudPane() {
  const providers = useAppStore((s) => s.cloudProviders);
  const refresh = useAppStore((s) => s.refreshCloudProviders);
  const [error, setError] = useState<string | null>(null);

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
        <ProviderSection key={p.id} provider={p} last={i === providers.length - 1} />
      ))}
    </div>
  );
}
