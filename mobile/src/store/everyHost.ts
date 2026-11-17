// Reading every paired host at once, for what is listed across them — Home's
// projects, the header's counts, the host list and what each host can run:
// each host's store read with one selector, and the hosts' projects merged into
// one list.

import { useCallback, useMemo, useSyncExternalStore } from "react";
import { useShallow } from "zustand/react/shallow";
import { type ProjectEntry, projectEntries } from "../lib/projects";
import type { RunsOnHost } from "../lib/runsOn";
import type { HostProtocol } from "../remote";
import { type HostState, hostStoreOf } from "./host";
import { appStore, useApp } from "./index";
import type { SavedHost } from "./persist";

/** The paired hosts' keys, in saved order. */
export function useHostKeys(): string[] {
  return useApp(useShallow((s) => s.hosts.map((h) => h.hostKey)));
}

/** `pick` over every paired host's store, in saved order: one value per host,
 *  in an array kept until one of the values changes. As with any selector,
 *  `pick` must hand back stable references. */
export function useEveryHost<U>(pick: (s: HostState, hostKey: string) => U): U[] {
  const hostKeys = useHostKeys();
  const stores = useMemo(
    () => hostKeys.map((hostKey) => ({ hostKey, store: hostStoreOf(hostKey) })),
    [hostKeys],
  );
  const subscribe = useCallback(
    (onChange: () => void) => {
      const offs = stores.map(({ store }) => store.subscribe(onChange));
      return () => {
        for (const off of offs) off();
      };
    },
    [stores],
  );
  const read = useShallow(() =>
    stores.map(({ hostKey, store }) => pick(store.getState(), hostKey)),
  );
  const snapshot = () => read(undefined);
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}

const workspaceOf = (s: HostState) => s.workspace;

/** Every paired host's projects as one list (`projectEntries`), in the hosts'
 *  own order. */
export function useProjectEntries(): ProjectEntry[] {
  const hostKeys = useHostKeys();
  const workspaces = useEveryHost(workspaceOf);
  return useMemo(
    () => projectEntries(hostKeys.map((hostKey, i) => ({ hostKey, workspace: workspaces[i] }))),
    [hostKeys, workspaces],
  );
}

const isConnected = (s: HostState) => s.connection === "connected";

/** The paired hosts that are connected, in saved order. */
export function useConnectedHosts(): SavedHost[] {
  const hosts = useApp((s) => s.hosts);
  const connected = useEveryHost(isConnected);
  return useMemo(() => hosts.filter((_, i) => connected[i]), [hosts, connected]);
}

/** What a connected host said it answers; undefined for one that is not. */
const connectedProtocol = (s: HostState) => (s.connection === "connected" ? s.protocol : undefined);

/** `hosts` as the "Runs on" rules weigh them (lib/runsOn), with `protocols`
 *  read off their stores in the same order: the connected ones only. */
const weigh = (
  hosts: readonly SavedHost[],
  protocols: readonly (HostProtocol | null | undefined)[],
): RunsOnHost[] =>
  hosts.flatMap((h, i) => {
    const protocol = protocols[i];
    return protocol === undefined ? [] : [{ hostKey: h.hostKey, name: h.name, protocol }];
  });

/** The connected paired hosts as the "Runs on" rules weigh them, in saved
 *  order. */
export function useRunsOnHosts(): RunsOnHost[] {
  const hosts = useApp((s) => s.hosts);
  const protocols = useEveryHost(connectedProtocol);
  return useMemo(() => weigh(hosts, protocols), [hosts, protocols]);
}

/** The same, read once: what an action weighs at the moment it runs. */
export function runsOnHostsNow(): RunsOnHost[] {
  const { hosts } = appStore.getState();
  return weigh(
    hosts,
    hosts.map((h) => connectedProtocol(hostStoreOf(h.hostKey).getState())),
  );
}
