// Agents the user has asked to remove (archive or discard) whose removal a
// workspace snapshot has not yet confirmed.
//
// The click is terminal: once the user archives an agent, no snapshot may put
// its row back — not the `workspace:changed` refetch of the previous click, not
// a foreground resync, not a fetch that started a moment before the backend
// committed. The generation guard in `refreshWorkspace` only drops fetches that
// a NEWER fetch superseded; it cannot tell that the newest fetch's response was
// produced before a later user action. So every snapshot applied through
// `refreshWorkspace` passes through `applyPendingHides`, which re-applies each
// pending hide until the snapshot itself shows the agent archived (or gone), at
// which point the entry is confirmed and dropped.
//
// A pending hide is withdrawn (`clearPendingHide`) only when the backend
// refuses the action — then nothing was archived and the row belongs back.

import type { AgentRecord, ArchiveMetadata, Workspace } from "@/api";

export type PendingHide = "archive" | "discard";

const pending = new Map<string, PendingHide>();

/** The `archive` stamped on a row hidden by a pending archive: the sidebar
 *  filters on `!a.archive`, so this drops the row immediately; the real
 *  metadata arrives with the confirming snapshot. Kept as one shared reference
 *  so the archive action can tell its own placeholder from authoritative data
 *  (`agent.archive === PENDING_ARCHIVE`) when deciding whether to undo. */
export const PENDING_ARCHIVE: ArchiveMetadata = {
  archived_at: "",
  repos: [],
  diff_stats: { additions: 0, deletions: 0 },
};

export const markPendingHide = (id: string, kind: PendingHide): void => {
  pending.set(id, kind);
};

export const clearPendingHide = (id: string): void => {
  pending.delete(id);
};

/** Confirm every pending hide `ws` already reflects (agent archived or gone),
 *  then re-apply the rest: a pending archive is stamped with `PENDING_ARCHIVE`,
 *  a pending discard is dropped from the list. Returns `ws` itself when there
 *  is nothing to do, so callers can pass snapshots through unconditionally. */
export const applyPendingHides = (ws: Workspace): Workspace => {
  if (pending.size === 0) return ws;
  const byId = new Map(ws.agents.map((a) => [a.id, a]));
  for (const id of [...pending.keys()]) {
    const agent = byId.get(id);
    if (!agent || agent.archive) pending.delete(id);
  }
  if (pending.size === 0) return ws;

  const agents: AgentRecord[] = [];
  for (const a of ws.agents) {
    const kind = pending.get(a.id);
    if (!kind) agents.push(a);
    else if (kind === "archive") agents.push({ ...a, archive: PENDING_ARCHIVE });
    // "discard": omitted — the row is gone as far as the UI is concerned.
  }
  return { ...ws, agents };
};
