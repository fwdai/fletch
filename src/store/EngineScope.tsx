import type { ReactNode } from "react";
import { EngineContext, engineOf } from "@/api/engine";
import type { EnvironmentId } from "./environments";
import { HostContext } from "./hostScope";

/** Scope a subtree to one host: `useAppStore` reads that host's state and runs
 *  its actions, and `useEngine` hands out its engine, so the two always agree.
 *  The app root mounts one for the environment on screen. */
export function EngineScope({ hostId, children }: { hostId: EnvironmentId; children: ReactNode }) {
  return (
    <HostContext.Provider value={hostId}>
      <EngineContext.Provider value={engineOf(hostId)}>{children}</EngineContext.Provider>
    </HostContext.Provider>
  );
}
