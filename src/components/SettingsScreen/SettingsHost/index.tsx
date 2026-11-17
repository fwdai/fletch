// Which machine Settings reads and writes host-owned settings for: the alerts
// and idle sweep, publishing, the sandbox, code indexing, provider binary paths
// (src/store/hostSettings.ts). This Mac unless the user picks a paired host in
// the pane; everything about this desktop alone stays This Mac's.

import { type ReactNode, useMemo, useState } from "react";
import { useAppStore } from "@/store";
import { EngineScope } from "@/store/EngineScope";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { SettingsHostContext, useSettingsHost } from "./context";

export { HostPicker } from "./HostPicker";

/** Holds the host Settings is showing for as long as the screen is open, so
 *  every visit starts on This Mac. A host forgotten meanwhile falls back to
 *  This Mac. */
export function SettingsHostProvider({ children }: { children: ReactNode }) {
  const [chosen, choose] = useState<EnvironmentId>(LOCAL_ENVIRONMENT_ID);
  const paired = useAppStore((s) => !!s.environments[chosen]);
  const hostId = paired ? chosen : LOCAL_ENVIRONMENT_ID;
  const value = useMemo(() => ({ hostId, choose }), [hostId]);
  return <SettingsHostContext.Provider value={value}>{children}</SettingsHostContext.Provider>;
}

/** A pane of host-owned settings, scoped to the host Settings is showing: its
 *  components read that host's state and write through its engine. Keyed by
 *  the host, so nothing the pane holds itself (a draft, an error, a read in
 *  flight) carries over to the next host. */
export function HostScope({ children }: { children: ReactNode }) {
  const { hostId } = useSettingsHost();
  return (
    <EngineScope key={hostId} hostId={hostId}>
      {children}
    </EngineScope>
  );
}
