import { useEffect, useState } from "react";
import type { HostActivity } from "@/api";
import { useEngine } from "@/api/engine";

/** The hosts VMs reached or were refused lately (`box_egress_activity`),
 *  for every project or one, on the host the caller is scoped to, kept live by
 *  `box:egress` (each answered connection, re-read after a short pause so a
 *  burst costs one read) and `box:policy`. `null` until the first read lands,
 *  and while the host cannot answer (`skip`). */
export function useEgressActivity(
  projectId: string | null,
  skip = false,
  hours?: number,
): HostActivity[] | null {
  const { api, events } = useEngine();
  const [hosts, setHosts] = useState<HostActivity[] | null>(null);

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
    const offs = [events.onBoxEgress(readSoon), events.onBoxPolicy(readSoon)];
    return () => {
      live = false;
      if (timer) clearTimeout(timer);
      for (const off of offs) void off.then((f) => f()).catch(() => {});
    };
  }, [projectId, skip, hours, api, events]);

  return hosts;
}
