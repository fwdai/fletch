import { api, type EgressDelta, type HostActivity } from "@/api";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { formatAge, formatBytes } from "@/util/format";
import { useEgressActivity } from "@/util/useEgressActivity";
import { SetGroup, SetRow } from "../../primitives";

/** Rows shown per list before the rest is left to the project pages. */
const MAX_ROWS = 12;

/** What VMs reached and what they were refused in the last day, across every
 *  project, from the host's record: the visibility an open posture rests on.
 *  A host reached outside the lists can be kept with one click, so a user who
 *  runs open for a while and then tightens has nothing to rediscover; a
 *  refused one can be allowed the same way. Both add to the global list, this
 *  being the page for every project. Telemetry is left out, and so are the
 *  hosts already on the user's list. */
export function RecentHosts({ global }: { global: EgressDelta }) {
  const gate = useGate("egressActivity");
  const hosts = useEgressActivity(null, !!gate);
  const agents = useAppStore((s) => s.workspace?.agents);
  if (gate || !hosts) return null;

  const allow = (host: string) =>
    void api.setBoxEgressGlobal({ add: [...global.add, host], remove: [] }).catch(() => {});
  const names = (ids: string[]) => {
    const first = agents?.find((a) => a.id === ids[0])?.name ?? "a workspace";
    return ids.length > 1 ? `${first} +${ids.length - 1}` : first;
  };
  const now = Date.now();
  const mine = new Set(global.add);
  const reached = hosts.filter((h) => h.allowed > 0 && !h.quiet && !mine.has(h.host));
  const blocked = hosts.filter((h) => h.allowed === 0 && h.refused > 0 && !h.quiet);

  return (
    <>
      {reached.length > 0 && (
        <SetGroup label="Reached recently">
          {reached.slice(0, MAX_ROWS).map((h) => (
            <SetRow
              key={h.host}
              compact
              title={<span className="mono">{h.host}</span>}
              sub={`${via(h)} · ${count(h.allowed)} · ${formatBytes(h.bytes_up)} up · ${names(h.agent_ids)} · ${formatAge(h.last_at, now)}`}
            >
              {h.last_via !== "listed" && (
                <Button
                  variant="ghost"
                  tip="Allow it in every project"
                  onClick={() => allow(h.host)}
                >
                  Allow
                </Button>
              )}
            </SetRow>
          ))}
        </SetGroup>
      )}
      {blocked.length > 0 && (
        <SetGroup label="Blocked recently">
          {blocked.slice(0, MAX_ROWS).map((h) => (
            <SetRow
              key={h.host}
              compact
              title={<span className="mono">{h.host}</span>}
              sub={`${h.last_via === "port" ? "port not allowed" : "refused"} ${count(h.refused, "time")} · ${names(h.agent_ids)} · ${formatAge(h.last_at, now)}`}
            >
              {h.last_via !== "port" && (
                <Button
                  variant="ghost"
                  tip="Allow it in every project"
                  onClick={() => allow(h.host)}
                >
                  Allow
                </Button>
              )}
            </SetRow>
          ))}
        </SetGroup>
      )}
    </>
  );
}

function count(n: number, noun = "connection"): string {
  return n === 1 ? `1 ${noun}` : `${n} ${noun}s`;
}

/** How the host got through, for the row. */
function via(h: HostActivity): string {
  switch (h.last_via) {
    case "listed":
      return "on a list";
    case "turn":
      return "opened for a turn";
    default:
      return "open network";
  }
}
