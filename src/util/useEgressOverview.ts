import { useCallback, useEffect, useState } from "react";
import type { EgressOverview } from "@/api";
import { useEngine } from "@/api/engine";

/** The template's categories, in display order, as the Network lists name
 *  them. Keys are `fletch_box::egress::Category::as_str`. */
export const EGRESS_CATEGORIES: { key: string; label: string }[] = [
  { key: "providers", label: "Agent providers" },
  { key: "registries", label: "Package registries" },
  { key: "container_registries", label: "Container registries" },
  { key: "toolchains", label: "Toolchains and CDNs" },
  { key: "source_hosting", label: "Source hosting" },
  { key: "os_packages", label: "OS packages" },
];

/** The egress allow-list layer by layer (`box_egress_overview`) on the host
 *  the caller is scoped to, with a project's own when `projectId` is given,
 *  kept live by `box:policy` and `project_file:changed`. `overview` is null
 *  until the first read lands, and stays null when the host cannot answer it
 *  (`skip`). */
export function useEgressOverview(
  projectId: string | null,
  skip = false,
): { overview: EgressOverview | null; reload: () => void } {
  const { api, events } = useEngine();
  const [overview, setOverview] = useState<EgressOverview | null>(null);
  const [nonce, setNonce] = useState(0);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: nonce is the intended re-run trigger
  useEffect(() => {
    if (skip) {
      setOverview(null);
      return;
    }
    let live = true;
    const read = () =>
      api
        .boxEgressOverview(projectId ?? undefined)
        .then((o) => live && setOverview(o))
        .catch(() => {});
    read();
    const offs = [
      events.onBoxPolicy((e) => {
        if (e.project_id === null || e.project_id === projectId) read();
      }),
      events.onProjectFileChanged((e) => {
        if (e.project_id === projectId) read();
      }),
    ];
    return () => {
      live = false;
      for (const off of offs) void off.then((f) => f()).catch(() => {});
    };
  }, [projectId, skip, nonce, api, events]);

  return { overview, reload };
}
