import { useEffect, useState } from "react";
import { api, type HostActivity, onBoxEgress, onBoxPolicy } from "@/api";
import { useAppStore } from "@/store";

/** The hosts VMs reached or were refused lately (`box_egress_activity`),
 *  for every project or one, kept live by `box:egress` (each answered
 *  connection, re-read after a short pause so a burst costs one read) and
 *  `box:policy`. `null` until the first read lands, and while the host cannot
 *  answer (`skip`). */
export function useEgressActivity(
  projectId: string | null,
  skip = false,
  hours?: number,
): HostActivity[] | null {
  const [hosts, setHosts] = useState<HostActivity[] | null>(null);
  const environmentId = useAppStore((s) => s.activeEnvironmentId);

  // biome-ignore lint/correctness/useExhaustiveDependencies: environmentId is the intended re-run trigger
  useEffect(() => {
    if (skip) {
      setHosts(null);
      return;
    }
    let live = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const read = () =>
      api
        .boxEgressActivity(projectId ?? undefined, hours)
        .then((rows) => live && setHosts(rows))
        .catch(() => {});
    const readSoon = () => {
      if (timer) return;
      timer = setTimeout(() => {
        timer = null;
        void read();
      }, 500);
    };
    void read();
    const offs = [onBoxEgress(readSoon), onBoxPolicy(readSoon)];
    return () => {
      live = false;
      if (timer) clearTimeout(timer);
      for (const off of offs) void off.then((f) => f()).catch(() => {});
    };
  }, [projectId, skip, hours, environmentId]);

  return hosts;
}
