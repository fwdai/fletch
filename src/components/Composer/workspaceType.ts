// The composer's workspace type for a new agent: this Mac's sandbox or an
// isolated VM. The host computes the options (`available_workspace_types`);
// these helpers only decide what the control shows and what the spawn carries,
// so the rule is the host's and the control cannot disagree with it.

import type { WorkspaceKind, WorkspaceTypeOption } from "@/api";

/** What a spawn with no choice gets, or null when neither option is it (the
 *  host pins a container engine in Settings). */
export function defaultWorkspaceType(options: WorkspaceTypeOption[]): WorkspaceKind | null {
  return options.find((o) => o.default)?.kind ?? null;
}

/** What the control shows as picked: the user's choice while the host still
 *  offers it, else what a spawn with no choice gets. */
export function shownWorkspaceType(
  options: WorkspaceTypeOption[],
  choice: WorkspaceKind | undefined,
): WorkspaceKind | null {
  if (choice && options.some((o) => o.kind === choice && o.available)) return choice;
  return defaultWorkspaceType(options);
}

/** What the spawn carries: the user's choice only while it is available and
 *  differs from the default. Picking the default sends nothing, so the spawn
 *  behaves exactly as it does without the control. */
export function workspaceTypeOverride(
  options: WorkspaceTypeOption[],
  choice: WorkspaceKind | undefined,
): WorkspaceKind | undefined {
  if (!choice) return undefined;
  const option = options.find((o) => o.kind === choice);
  if (!option?.available || option.default) return undefined;
  return choice;
}

/** The words the composer and the project page use for each type. */
export function workspaceTypeLabel(kind: WorkspaceKind): string {
  return kind === "box" ? "Isolated VM" : "This Mac";
}
