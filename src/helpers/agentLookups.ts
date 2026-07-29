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

/** Names held by open drafts — the only reserved names the backend can't see
 *  for itself, since a draft isn't persisted until it spawns. Everything else
 *  taken is a live agent, which `allocate_draft_name` reads from the DB.
 *
 *  This used to also fold in `workspace.agents`, but that list carries archived
 *  agents (History reads the same one), so every archive permanently burned a
 *  slot in the ~300-name pool and pushed new workspaces onto `-2` suffixes.
 *  Sending agents at all was the mistake — the DB already knows which are
 *  live, so the frontend no longer gets a say. */
export function draftNames(drafts: DraftAgent[]): string[] {
  return drafts.map((d) => d.name);
}

/** Drop an agent's entries from a repo-scoped map: the plain `id` key (the
 *  primary repo) plus any `id::subdir` composite keys a multi-repo agent's
 *  per-repo fetches and bulk polls wrote (see `checkoutKey` in store/git). */
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
  // The git/PR/delegation maps are checkout-scoped: a multi-repo agent also
  // holds `id::subdir` keys, which must not outlive it.
  const gitStates = dropScopedEntries(state.gitStates, id);
  const prStates = dropScopedEntries(state.prStates, id);
  const prChecks = dropScopedEntries(state.prChecks, id);
  const prComments = dropScopedEntries(state.prComments, id);
  const delegations = dropScopedEntries(state.delegations, id);
  const delegationNotices = dropScopedEntries(state.delegationNotices, id);
  const autopilot = dropScopedEntries(state.autopilot, id);
  const autopilotVerdicts = dropScopedEntries(state.autopilotVerdicts, id);
  const { [id]: _short, ...gitShortstats } = state.gitShortstats;
  const { [id]: _seed, ...composerSeeds } = state.composerSeeds;
  const { [id]: _draft, ...composerDrafts } = state.composerDrafts;
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
    delegations,
    delegationNotices,
    autopilot,
    autopilotVerdicts,
    unseenResults,
    rightPanelTabs,
  };
}
