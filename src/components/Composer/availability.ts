import { isDockerSupported, providerLabel } from "@/data/providers";
import { useAppStore } from "@/store";

export interface Availability {
  /** Why this agent can't be spawned right now, or null when it can. Rendered
   *  as the row's refusal tooltip. */
  reason: string | null;
  /** The short status word the row carries on its right: the probed CLI version
   *  normally, the refusal in brief when there is one. */
  note: string;
}

/** Whether an agent can be spawned under the current settings, and why not.
 *
 *  Two gates, both mirroring the backend so the picker and the spawn path can't
 *  disagree: the provider CLI must actually be installed (`providerPaths`, from
 *  the startup probe), and under the Docker engine it must be one of the
 *  container-ready providers (`isDockerSupported`, mirroring
 *  `ensure_engine_supports_provider`).
 *
 *  Shared by every surface that offers an agent — the composer's picker and the
 *  Roadmap's new-chat screen — because a rule that decides what the user is
 *  allowed to start must not exist twice.
 *
 *  Pass a custom agent's `base` to gate it: a custom agent inherits its base
 *  provider's availability exactly. */
export function useAgentAvailability(): (providerId: string) => Availability {
  const providerVersions = useAppStore((s) => s.providerVersions);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const providersProbed = useAppStore((s) => s.providersProbed);
  // New agents get the currently selected sandbox engine, so a provider without
  // container support is unspawnable while Docker is on.
  const dockerOnly = useAppStore((s) => s.sandboxEngine) === "docker";

  return (providerId: string) => {
    // dockerBlocked is checked first: a non-container provider under Docker is
    // blocked regardless of install state, and that's the more useful reason.
    if (dockerOnly && !isDockerSupported(providerId)) {
      return {
        reason: `${providerLabel(providerId)} isn't available in Docker sandboxes yet`,
        note: "Not in Docker yet",
      };
    }
    // Fail open on the install gate: only enforce it once a probe has actually
    // succeeded (`providersProbed`). While probing, or if the probe failed,
    // treat as installed so a transient detection error never disables an agent
    // the user really has.
    if (providersProbed && !providerPaths[providerId]) {
      return { reason: "Not installed — see Settings › Providers", note: "Not installed" };
    }
    return { reason: null, note: providerVersions[providerId] ?? "" };
  };
}
