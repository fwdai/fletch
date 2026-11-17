import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { WfRun } from "@/api";
import { useAppStore, useEveryHost } from "@/store";
import { EngineScope } from "@/store/EngineScope";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { useRuns } from "@/workflows/run/useRuns";

type Report = (hostId: EnvironmentId, runs: WfRun[] | null) => void;

/** Every host's workflow runs, by host. `useRuns` keeps a host's runs in
 *  component state rather than the store, so This Mac's are read here and
 *  each paired host's by a `RunsFeed` of its own, in that host's scope, which
 *  reports them back. `feeds` renders those; it draws nothing. */
export function useHostRuns(): {
  runsByHost: Readonly<Record<EnvironmentId, readonly WfRun[]>>;
  feeds: ReactNode;
} {
  const localRuns = useRuns();
  const hostIds = useEveryHost((_s, hostId) => hostId);
  const [pairedRuns, setPairedRuns] = useState<Record<EnvironmentId, WfRun[]>>({});
  const report = useCallback<Report>(
    (hostId, runs) =>
      setPairedRuns((m) => {
        if (runs) return { ...m, [hostId]: runs };
        const { [hostId]: _gone, ...rest } = m;
        return rest;
      }),
    [],
  );
  const runsByHost = useMemo(
    () => ({ ...pairedRuns, [LOCAL_ENVIRONMENT_ID]: localRuns }),
    [pairedRuns, localRuns],
  );
  const feeds = hostIds
    .filter((hostId) => hostId !== LOCAL_ENVIRONMENT_ID)
    .map((hostId) => (
      <EngineScope key={hostId} hostId={hostId}>
        <RunsFeed hostId={hostId} report={report} />
      </EngineScope>
    ));
  return { runsByHost, feeds };
}

/** One paired host's runs, read again each time its connection comes up: a
 *  read made before the first handshake, or while the host was away, has
 *  nothing to answer it, and the events since were not heard. The rows held
 *  stay on show in between. */
function RunsFeed({ hostId, report }: { hostId: EnvironmentId; report: Report }) {
  const connected = useAppStore((s) => s.environments[hostId]?.connection === "connected");
  const wasConnected = useRef(connected);
  const [connections, setConnections] = useState(0);
  useEffect(() => {
    if (connected && !wasConnected.current) setConnections((n) => n + 1);
    wasConnected.current = connected;
  }, [connected]);
  const runs = useRuns(connections);
  useEffect(() => report(hostId, runs), [hostId, runs, report]);
  useEffect(() => () => report(hostId, null), [hostId, report]);
  return null;
}
