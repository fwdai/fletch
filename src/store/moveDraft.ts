// A draft moved to another host: its project is made ready there, then the
// draft is opened again in that host's scope.
//
// A draft runs on the host it lives on. Picking another host on the
// new-workspace row therefore moves the draft, and from then on everything
// the screen offers (the agents, the accounts, the name, where files are
// staged) is that host's own, and the first send is an ordinary spawn there.
//
// The draft's host describes the project (`project_definition`); the target
// makes sure it can run it (`ensure_project`, which clones the repositories
// the first time) and answers with its copy; the target then opens a draft of
// its own on that copy, with what the user has typed and the choices that mean
// the same on any device (`CarriedDraft`). All of that happens before anything
// is sent, so a failure leaves the draft, and its message, where they were.

import { type Engine, engineOf } from "@/api/engine";
import { preparingLabel, runTargetOptions } from "@/components/Composer/RunTargetPicker/options";
import type { EnvironmentId } from "./environments";
import { otherHosts } from "./otherHosts";
import { refreshWorkspace } from "./refreshWorkspace";
import type { AppState, SliceCreator } from "./types";

type Set = Parameters<SliceCreator<AppState>>[0];

const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));

/** Move the draft `id` from its own host (`own`) to `target`. A roadmap
 *  hand-off stays on its own host, where its item is linked to the agent it
 *  spawns. */
export async function moveDraft(
  own: { engine: Engine; get: () => AppState; set: Set },
  id: string,
  target: EnvironmentId,
): Promise<void> {
  const { engine, get, set } = own;
  const { hostStore, onHost } = otherHosts();
  const draft = get().drafts.find((d) => d.id === id);
  if (!draft || draft.preparing || draft.roadmapItemId || target === engine.hostId) return;
  const host = get().environments[target];
  const project = get().workspace?.projects.find((p) => p.path === draft.repoPath);
  if (!host || !project) {
    set({ lastError: "Couldn't find this draft's project." });
    return;
  }
  set({ lastError: null });
  get().updateDraft(id, { preparing: preparingLabel(host.name) });
  try {
    const definition = await engine.api.projectDefinition(project.project_id);
    // The picker's own rule, read again now that the definition is in hand:
    // the host may have gone since the pick.
    const option = runTargetOptions({
      environments: get().environments,
      draftHost: engine.hostId,
      definition,
    }).find((o) => o.hostId === target);
    if (option?.reason) throw new Error(option.reason);

    const there = hostStore(target);
    const copy = await engineOf(target).api.ensureProject(definition);
    const repoPath = copy.projects.find((p) => p.project_id === definition.project_id)?.path;
    if (!repoPath) throw new Error(`${host.name} has no copy of ${definition.name}.`);
    // Shown through the one path that orders a host's snapshots.
    await refreshWorkspace(engineOf(target), there.setState);

    const now = get().drafts.find((d) => d.id === id);
    if (!now) return;
    const moved = await there.getState().createDraft(repoPath, {
      seedPrompt: get().composerDrafts[id],
      kind: now.kind,
      carried: {
        provider: now.provider,
        model: now.model,
        base: now.base,
        issueRef: now.issueRef,
      },
      // Asked as the target is about to open it: a draft discarded at any
      // point of the move opens nothing there.
      when: () => get().drafts.some((d) => d.id === id),
    });
    // Not opened: discarded, or the target says why in the banner, which the
    // window shares.
    if (!moved) return;
    // Read with the new draft just opened, so nothing the user did while
    // the target was asked is overridden: the moved draft takes the screen
    // only if the old one still has it.
    const onScreen = get().activeEnvironmentId === engine.hostId && get().activeDraftId === id;
    get().removeDraft(id);
    if (onScreen) onHost(target);
  } catch (e) {
    set({ lastError: message(e) });
  } finally {
    // A no-op once the draft has left.
    get().updateDraft(id, { preparing: undefined });
  }
}
