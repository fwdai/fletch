import type { EgressDelta, HostActivity } from "@/api";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { canAllow, countsLabel, hostStanding, viaLabel } from "@/util/egressActivity";
import { formatAge, formatBytes } from "@/util/format";
import { useEgressActivity } from "@/util/useEgressActivity";
import { SetDisclosure, SetGroup, SetRow } from "../../primitives";
import { globalHosts, type HostList } from "./hostList";

/** Rows shown per list before the rest is left to the project pages. */
const MAX_ROWS = 12;

/** What VMs reached and what they were refused in the last day, from the
 *  host's record: the visibility an open posture rests on. Globally across
 *  every project, one group each; on a project's page that project's alone,
 *  folded away, and only what reached beyond its lists. A host sits in the
 *  list its newest connection put it in (`hostStanding`), with both counts
 *  when its window holds both. A host reached outside the lists can be kept
 *  with one click, so a user who runs open for a while and then tightens has
 *  nothing to rediscover; a refused one can be allowed the same way. Both add
 *  to the list the page edits. Telemetry is left out, and so are the hosts
 *  already on that list. */
export function RecentHosts(props: { global: EgressDelta } | { project: HostList }) {
  const list = "project" in props ? props.project : globalHosts(props.global);
  const projectId = list.projectId ?? null;
  const gate = useGate("egressActivity");
  const hosts = useEgressActivity(projectId, !!gate);
  const agents = useAppStore((s) => s.workspace?.agents);
  if (gate || !hosts) return null;

  const allow = list.allow;
  const names = (ids: string[]) => {
    const first = agents?.find((a) => a.id === ids[0])?.name ?? "a workspace";
    return ids.length > 1 ? `${first} +${ids.length - 1}` : first;
  };
  const now = Date.now();
  const mine = new Set(list.hosts.map((h) => h.pattern));
  const shown = hosts.filter((h) => !h.quiet && !mine.has(h.host));
  const reached = {
    label: projectId ? "Reached beyond the list" : "Reached recently",
    hosts: shown.filter(
      (h) => hostStanding(h) === "reached" && !(projectId && h.last_via === "listed"),
    ),
  };
  const blocked = {
    label: "Blocked recently",
    hosts: shown.filter((h) => hostStanding(h) === "blocked"),
  };
  const groups = [reached, blocked].filter((g) => g.hosts.length > 0);

  const row = (h: HostActivity) => (
    <SetRow
      key={h.host}
      compact
      title={<span className="mono">{h.host}</span>}
      sub={[
        viaLabel(h.last_via),
        countsLabel(h),
        h.bytes_up > 0 ? `${formatBytes(h.bytes_up)} up` : null,
        names(h.agent_ids),
        formatAge(h.last_at, now),
      ]
        .filter(Boolean)
        .join(" · ")}
    >
      {allow && canAllow(h) && (
        <Button
          variant="ghost"
          tip={`Allow it in ${projectId ? "this" : "every"} project`}
          onClick={() => void allow(h.host).catch(() => {})}
        >
          Allow
        </Button>
      )}
    </SetRow>
  );

  if (projectId) {
    // The egress toast's "Review" lands on this pane: refused hosts stay in
    // view with their Allow, and only what was merely reached folds away.
    return (
      <>
        {blocked.hosts.length > 0 && (
          <SetGroup label={blocked.label}>{blocked.hosts.map(row)}</SetGroup>
        )}
        {reached.hosts.length > 0 && (
          <SetDisclosure label={`${reached.label} · ${reached.hosts.length} hosts`}>
            {reached.hosts.map(row)}
          </SetDisclosure>
        )}
      </>
    );
  }
  return (
    <>
      {groups.map((g) => (
        <SetGroup key={g.label} label={g.label}>
          {g.hosts.slice(0, MAX_ROWS).map(row)}
        </SetGroup>
      ))}
    </>
  );
}
