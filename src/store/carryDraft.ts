// A draft whose agent runs on a host other than its own: its project is carried
// there, then the agent is spawned by that host's own actions.
//
// The draft's host describes the project (`project_definition`); the target
// makes sure it can run it (`ensure_project`, which clones the repositories the
// first time) and answers with its copy; the target's `spawnFromDraft` then
// spawns from the draft as the target reads it (`HandedDraft`), so the new
// workspace, its first message and everything after land in the target's
// partition. The draft leaves its own host only once the agent exists: a
// failure before that leaves it where it was, with the error in the banner.

import { type Engine, engineOf } from "@/api/engine";
import { preparingLabel, runTargetOptions } from "@/components/Composer/RunTargetPicker/options";
import type { DraftAgent } from "./drafts";
import type { EnvironmentId } from "./environments";
import { otherHosts } from "./otherHosts";
import type { AppState, SliceCreator } from "./types";

type Set = Parameters<SliceCreator<AppState>>[0];

/** The message that spawns the agent, as the composer sent it. */
export interface DraftMessage {
  text: string;
  provider: string;
  model?: string;
  attachments: string[];
  thinking?: string;
  customAgentId?: string;
}

/** `draft` as the host it runs on reads it: its project at the copy's path
 *  there, and nothing that names something of the draft's own host. The
 *  account and the workspace type are that host's choices, as on a paired
 *  host's own draft, and a roadmap item is linked on the host that has it. */
function handedDraft(draft: DraftAgent, repoPath: string): DraftAgent {
  return {
    id: draft.id,
    kind: draft.kind,
    repoPath,
    name: draft.name,
    provider: draft.provider,
    model: draft.model,
    customAgentId: draft.customAgentId,
    base: draft.base,
    issueRef: draft.issueRef,
    afterProjectAdd: draft.afterProjectAdd,
  };
}

const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));

/** Spawn `draft`'s agent on `target` from the draft's own host (`own`).
 *  Resolves true once the agent exists there, false when it does not (the
 *  reason is in `lastError`, and the draft is back to taking a message).
 *  Folders are never carried: a grant names a path on the draft's host. */
export async function carryDraft(
  own: { engine: Engine; get: () => AppState; set: Set },
  draft: DraftAgent,
  target: EnvironmentId,
  sent: DraftMessage,
): Promise<boolean> {
  const { engine, get, set } = own;
  const { hostStore, onHost } = otherHosts();
  const host = get().environments[target];
  const project = get().workspace?.projects.find((p) => p.path === draft.repoPath);
  if (!host || !project) {
    set({ lastError: "Couldn't find this draft's project." });
    return false;
  }
  get().setLastRepoPath(draft.repoPath);
  set({ busy: true, lastError: null });
  get().updateDraft(draft.id, { preparing: preparingLabel(host.name) });
  let spawned = false;
  try {
    const definition = await engine.api.projectDefinition(project.project_id);
    // The picker's own rule, read again now that the definition is in hand:
    // the host may have gone, or the provider been switched, since the pick.
    const option = runTargetOptions({
      environments: get().environments,
      draftHost: engine.hostId,
      provider: sent.provider,
      definition,
    }).find((o) => o.hostId === target);
    if (option?.reason) throw new Error(option.reason);

    const there = hostStore(target);
    const { api } = engineOf(target);
    const workspace = await api.ensureProject(definition);
    there.setState({ workspace });
    const repoPath = workspace.projects.find((p) => p.project_id === definition.project_id)?.path;
    if (!repoPath) throw new Error(`${host.name} has no copy of ${definition.name}.`);
    // The draft's base goes as it is. The host forks from the branch as its
    // remote has it, and fails the spawn for one that was never pushed
    // rather than start somewhere else; a branch list read here would only
    // name the copy's local branches, which is one.

    await there
      .getState()
      .spawnFromDraft(
        draft.id,
        sent.text,
        sent.provider,
        sent.model,
        sent.attachments,
        sent.thinking,
        sent.customAgentId,
        [],
        {
          draft: handedDraft(draft, repoPath),
          onSpawned: () => {
            spawned = true;
            get().removeDraft(draft.id);
            onHost(target);
          },
        },
      );
  } catch (e) {
    set({ lastError: message(e) });
  } finally {
    if (!spawned) get().updateDraft(draft.id, { preparing: undefined });
    set({ busy: false });
  }
  return spawned;
}
