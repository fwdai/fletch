import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import type { WfRun } from "@/api";
import { useEveryHost } from "@/store";
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

function RunsFeed({ hostId, report }: { hostId: EnvironmentId; report: Report }) {
  const runs = useRuns();
  useEffect(() => report(hostId, runs), [hostId, runs, report]);
  useEffect(() => () => report(hostId, null), [hostId, report]);
  return null;
}
