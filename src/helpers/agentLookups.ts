// Pure lookups and per-agent state derivations over the store shape
// (AppState/Workspace/DraftAgent). Type-only store import, erased at compile
// time, so there's no runtime cycle.

import type { Workspace } from "../api";
import type { AppState, DraftAgent } from "../store";

export function providerFor(state: AppState, agentId: string): string | undefined {
  return state.workspace?.agents.find((a) => a.id === agentId)?.provider;
}

/** The primary repo path for an agent (`repos[0]`), used to scope
 *  project-level slash-command discovery. Undefined for an unknown agent. */
export function repoPathFor(state: AppState, agentId: string): string | undefined {
  return state.workspace?.agents.find((a) => a.id === agentId)?.repos[0]?.repo_path;
}

/** A per-turn agent captures its session id on its first turn (e.g. agy reads
 *  it from disk at turn-end), but the id only reaches the live frontend via a
 *  full `getWorkspace`. True when an agent's turn just landed yet its session
 *  id is still missing locally — the cue to re-fetch so the Native toggle
 *  unblocks without a reload. False once present, to avoid per-turn re-fetch. */
export function needsSessionIdRefresh(workspace: Workspace | null, agentId: string): boolean {
  const agent = workspace?.agents.find((a) => a.id === agentId);
  return !!agent && !agent.session_id;
}

/** Names already taken by real or draft agents — passed to the backend
 *  name allocator so picks avoid collisions. */
export function usedNames(workspace: Workspace | null, drafts: DraftAgent[]): Set<string> {
  const used = new Set<string>();
  for (const a of workspace?.agents ?? []) used.add(a.name);
  for (const d of drafts) used.add(d.name);
  return used;
}

/** Drop an agent's entries from a repo-scoped map: the plain `id` key (the
 *  primary repo) plus any `id::subdir` composite keys a multi-repo agent's
 *  per-repo fetches and bulk polls wrote (see `gitKey` in store/git). */
function dropScopedEntries<T>(map: Record<string, T>, id: string): Record<string, T> {
  const prefix = `${id}::`;
  return Object.fromEntries(
    Object.entries(map).filter(([key]) => key !== id && !key.startsWith(prefix)),
  );
}

/** Strip an agent's entries from every ephemeral per-agent map, returning just
 *  the pruned maps as a state patch (the caller layers on workspace /
 *  selectedAgentId). Shared by discard and archive — dropping these is safe
 *  because History re-loads an archived agent's transcript fresh from disk. */
export function dropAgentEntries(state: AppState, id: string): Partial<AppState> {
  const { [id]: _log, ...managedLogs } = state.managedLogs;
  const { [id]: _loading, ...transcriptLoading } = state.transcriptLoading;
  const { [id]: _loaded, ...transcriptLoaded } = state.transcriptLoaded;
  const { [id]: _busy, ...managedBusy } = state.managedBusy;
  const { [id]: _started, ...turnStartedAt } = state.turnStartedAt;
  const { [id]: _usage, ...usage } = state.usage;
  // The git/PR maps are repo-scoped: a multi-repo agent also holds
  // `id::subdir` keys, which must not outlive it.
  const gitStates = dropScopedEntries(state.gitStates, id);
  const prStates = dropScopedEntries(state.prStates, id);
  const prChecks = dropScopedEntries(state.prChecks, id);
  const prComments = dropScopedEntries(state.prComments, id);
  const { [id]: _short, ...gitShortstats } = state.gitShortstats;
  const { [id]: _seed, ...composerSeeds } = state.composerSeeds;
  const { [id]: _draft, ...composerDrafts } = state.composerDrafts;
  const { [id]: _delegation, ...gitDelegations } = state.gitDelegations;
  // Drop the unseen-results flag too: otherwise archiving/discarding an agent
  // that finished while unviewed leaves an orphaned key behind with no row to
  // select, which would keep the app-icon badge count nonzero forever.
  const { [id]: _seen, ...unseenResults } = state.unseenResults;
  // Drop the remembered right-rail tab so an archived/discarded agent's UI
  // state doesn't outlive it as a stale key for the rest of the session.
  const { [id]: _tab, ...rightPanelTabs } = state.rightPanelTabs;
  return {
    managedLogs,
    transcriptLoading,
    transcriptLoaded,
    managedBusy,
    turnStartedAt,
    usage,
    gitStates,
    gitShortstats,
    prStates,
    prChecks,
    prComments,
    composerSeeds,
    composerDrafts,
    gitDelegations,
    unseenResults,
    rightPanelTabs,
  };
}
