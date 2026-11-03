// The workspace type for a new workspace: a Linux VM or the macOS sandbox,
// picked on the new-workspace row (`WorkspaceTypePicker`) beside the project
// and the branch. The chip declares where the agent runs: the spawn carries exactly
// the kind the chip shows, the user's pick or the host's default alike, so the
// host lands the agent there or refuses with why, and the display cannot
// disagree with the outcome. The options are the host's
// (`available_workspace_types`), read for one set of inputs and never shown
// for others; while there is no such answer nothing is shown and nothing sent.

import type { WorkspaceKind, WorkspaceTypeOption } from "@/api";
import type { IconName } from "@/components/Icon";
import { LINUX_VM, MACOS_SANDBOX } from "@/util/sandboxNames";

/** The option the chip shows: the user's explicit pick whenever the host
 *  lists it — even when it is the default, or no longer available (the spawn
 *  then fails with its reason rather than landing elsewhere) — else the
 *  default. */
export function shownWorkspaceType(
  options: WorkspaceTypeOption[],
  choice: WorkspaceKind | undefined,
): WorkspaceTypeOption | null {
  return (
    (choice && options.find((o) => o.kind === choice)) || options.find((o) => o.default) || null
  );
}

/** The words the composer uses for each type. */
export function workspaceTypeLabel(kind: WorkspaceKind): string {
  return kind === "box" ? LINUX_VM.label : MACOS_SANDBOX.label;
}

export function workspaceTypeIcon(kind: WorkspaceKind): IconName {
  return kind === "box" ? LINUX_VM.icon : MACOS_SANDBOX.icon;
}

/** The option the chip declares for the host's answer, if there is one. The
 *  chip draws it and the spawn sends its kind, so the two are one derivation. */
export function declaredWorkspaceType(
  options: WorkspaceTypeOption[] | null,
  choice: WorkspaceKind | undefined,
): WorkspaceTypeOption | null {
  return options ? shownWorkspaceType(options, choice) : null;
}

/** What the spawn carries: the kind the chip declares, the default included,
 *  while the control is offered (an agent on this Mac: a paired host's spawn
 *  carries no workspace type) and `options` is the host's answer for the
 *  draft's current inputs. Nothing otherwise (still loading, the host could
 *  not answer): the host decides. */
export function workspaceTypeToSend(
  choice: WorkspaceKind | undefined,
  offered: boolean,
  options: WorkspaceTypeOption[] | null,
): WorkspaceKind | undefined {
  return offered ? declaredWorkspaceType(options, choice)?.kind : undefined;
}

/** The inputs an answer is for, as one key: the repo and the provider. An
 *  answer is shown, and sent, only for its own key. */
export function workspaceTypesKey(repoPath: string, provider: string): string {
  return [repoPath, provider].join("\u0000");
}

/** One successful read: the options and the key of the inputs it was for. */
export interface WorkspaceTypeAnswer {
  key: string;
  options: WorkspaceTypeOption[];
}

/** The answer's options when it is for `key`, else null. */
export function answeredOptions(
  answer: WorkspaceTypeAnswer | null,
  key: string,
): WorkspaceTypeOption[] | null {
  return answer?.key === key ? answer.options : null;
}

/** Whether a read's response may land: it is the latest one started and its
 *  key is the inputs the picker on screen has now (`null` when none is). */
export function workspaceTypeResponseLands(
  request: { id: number; key: string },
  latestId: number,
  currentKey: string | null,
): boolean {
  return request.id === latestId && request.key === currentKey;
}

export const WORKSPACE_TYPES_UNREADABLE = "Couldn't load the workspace types. Click to retry.";

/** The default macOS sandbox's reason while the VM image downloads: the
 *  host's `sandbox::IMAGE_DOWNLOADING`, to which the tip adds the live
 *  percent the host's answer cannot carry. */
export const IMAGE_DOWNLOADING = "VM image downloading";

/** The chip's one-sentence tooltip for the option it shows. The why comes
 *  from the host (`reason`, set on the default where it is worth saying, and
 *  on an unavailable option); `progress` is the image download's live
 *  percent. */
export function workspaceTypeTip(
  shown: WorkspaceTypeOption,
  progress: number | null | undefined,
): string {
  if (!shown.available) return shown.reason ?? `${workspaceTypeLabel(shown.kind)} unavailable`;
  const runs =
    shown.kind === "box" ? `Runs in a ${LINUX_VM.label}` : `Runs in the ${MACOS_SANDBOX.label}`;
  if (!shown.reason) return runs;
  const pct = shown.reason === IMAGE_DOWNLOADING && progress != null ? ` ${progress}%` : "";
  return `${runs} · ${shown.reason}${pct}`;
}

/** What the chip draws: nothing (no answer for the current inputs yet), a
 *  retry chip (no answer for these inputs and the last read failed; it shows
 *  the pick if there is one, and the menu stays shut), or the menu chip for
 *  the declared option. */
export type WorkspaceTypeChip =
  | null
  | { kind: "retry"; pick: WorkspaceKind | null; tip: string }
  | {
      kind: "menu";
      options: WorkspaceTypeOption[];
      shown: WorkspaceTypeOption;
      tip: string;
    };

export function workspaceTypeChip(
  options: WorkspaceTypeOption[] | null,
  failed: boolean,
  choice: WorkspaceKind | undefined,
  progress?: number | null,
): WorkspaceTypeChip {
  if (!options) {
    if (!failed) return null;
    return { kind: "retry", pick: choice ?? null, tip: WORKSPACE_TYPES_UNREADABLE };
  }
  const shown = declaredWorkspaceType(options, choice);
  if (!shown) return null;
  return { kind: "menu", options, shown, tip: workspaceTypeTip(shown, progress) };
}
