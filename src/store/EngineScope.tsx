import type { ReactNode } from "react";
import { EngineContext, engineOf } from "@/api/engine";
import type { EnvironmentId } from "./environments";
import { HostContext } from "./hostScope";
import { useAppStore, useEveryHost } from "./index";

/** Scope a subtree to one host: `useAppStore` reads that host's state and runs
 *  its actions, and `useEngine` hands out its engine, so the two always agree.
 *  Outside one, the subtree is This Mac's. */
export function EngineScope({ hostId, children }: { hostId: EnvironmentId; children: ReactNode }) {
  return (
    <HostContext.Provider value={hostId}>
      <EngineContext.Provider value={engineOf(hostId)}>{children}</EngineContext.Provider>
    </HostContext.Provider>
  );
}

/** Scope a subtree to the host on screen: the main pane, and whatever follows
 *  what it shows. Mounted afresh for each host that comes on screen: agent ids
 *  and repo paths repeat across hosts, and one host's local state (a scroll
 *  position, a half-typed field) is nothing to show over another's. */
export function OnScreenScope({ children }: { children: ReactNode }) {
  const hostId = useAppStore((s) => s.activeEnvironmentId);
  return (
    <EngineScope key={hostId} hostId={hostId}>
      {children}
    </EngineScope>
  );
}

/** `children` once per host, each in that host's scope: This Mac's first, then
 *  every paired host's. For what has to run or show for every host at once
 *  (a poll, a prompt, a toast), whichever one is on screen. */
export function EachHostScope({ children }: { children: ReactNode }) {
  const hostIds = useEveryHost((_s, hostId) => hostId);
  return (
    <>
      {hostIds.map((hostId) => (
        <EngineScope key={hostId} hostId={hostId}>
          {children}
        </EngineScope>
      ))}
    </>
  );
}
