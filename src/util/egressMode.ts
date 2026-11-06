import { type EgressMode, parseEgressMode } from "@/api";

/** Project-settings key of a project's own posture (`sandbox.egress_mode`),
 *  over the global `box_egress_mode`. Absent is the global. */
export const EGRESS_MODE_KEY = "sandbox.egress_mode";

/** The three postures as the controls name them, in order of openness. */
export const EGRESS_MODE_LABELS: Record<EgressMode, string> = {
  open: "Open",
  ask: "Ask me",
  allowlist: "Allow list",
};

/** A few words on what each posture does, for the picker's description. */
export const EGRESS_MODE_HINTS: Record<EgressMode, string> = {
  open: "Any host passes and is recorded below.",
  ask: "The agent waits up to a minute for your answer.",
  allowlist: "Anything off the list is refused at once.",
};

/** The posture a project's VMs run under: its own override when set, else the
 *  global setting. Both raw, as the settings maps hold them. */
export function effectiveEgressMode(
  projectRaw: string | undefined,
  globalRaw: string | undefined,
): { mode: EgressMode; overridden: boolean } {
  if (projectRaw) return { mode: parseEgressMode(projectRaw), overridden: true };
  return { mode: parseEgressMode(globalRaw), overridden: false };
}
