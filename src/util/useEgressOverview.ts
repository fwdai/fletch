import { useCallback, useEffect, useState } from "react";
import { api, type EgressOverview, onBoxPolicy, onProjectFileChanged } from "@/api";
import { useAppStore } from "@/store";

/** The template's categories, in display order, as the Network lists name
 *  them. Keys are `fletch_box::egress::Category::as_str`. */
export const EGRESS_CATEGORIES: { key: string; label: string }[] = [
  { key: "providers", label: "Agent providers" },
  { key: "registries", label: "Package registries" },
  { key: "toolchains", label: "Toolchains and CDNs" },
  { key: "source_hosting", label: "Source hosting" },
  { key: "os_packages", label: "OS packages" },
];

/** The egress allow-list layer by layer (`box_egress_overview`), with a
 *  project's own when `projectId` is given, kept live by `box:policy` and
 *  `project_file:changed`. `overview` is null until the first read lands, and
 *  stays null when the host cannot answer it (`skip`). */
export function useEgressOverview(
  projectId: string | null,
  skip = false,
): { overview: EgressOverview | null; reload: () => void } {
  const [overview, setOverview] = useState<EgressOverview | null>(null);
  const [nonce, setNonce] = useState(0);
  const environmentId = useAppStore((s) => s.activeEnvironmentId);
  const reload = useCallback(() => setNonce((n) => n + 1), []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: environmentId and nonce are the intended re-run triggers
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
      onBoxPolicy((e) => {
        if (e.project_id === null || e.project_id === projectId) read();
      }),
      onProjectFileChanged((e) => {
        if (e.project_id === projectId) read();
      }),
    ];
    return () => {
      live = false;
      for (const off of offs) void off.then((f) => f()).catch(() => {});
    };
  }, [projectId, skip, environmentId, nonce]);

  return { overview, reload };
}
