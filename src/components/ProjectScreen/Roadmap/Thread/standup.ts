// The standup digest: the one unprompted message a reopened PM chat gets when
// the board moved while nobody was looking.
//
// Why it exists: the board is autonomous. Runs dispatch, PRs open, the merge
// sweep ships things — all with the Roadmap tab closed. So the conversation the
// user comes back to is stale in a way neither party can see from the transcript,
// and the user ends up typing "what happened?" every single time. This asks it
// for them, once, from the board rather than from memory.
//
// Why it is a *pure* decision: firing this wrongly is worse than not firing it.
// A digest on a chat where nothing changed is noise the user learns to ignore; a
// second one in the same sitting reads as a bug; one on a chat that was created
// seconds ago asks an agent to summarize a board it has never seen. Each of
// those is a rule about timestamps and nothing else, so they live here with
// tests rather than tangled into an effect.

import type { AgentRecord, UserTurn } from "@/api";

/** The message the digest sends. Ends on the tool call so the answer comes from
 *  the board and not from the transcript — the whole point is that the board
 *  moved without this conversation hearing about it. */
export const STANDUP_PROMPT =
  "Summarize what shipped, failed, or got blocked since we last spoke, then " +
  "recommend what to queue next — check roadmap_list first.";

/** Everything the decision turns on. All times are epoch millis. */
export interface StandupSignals {
  /** When the board last moved — the newest item-event's timestamp. `null` for a
   *  board with no history at all, which is a board with nothing to summarize. */
  boardMovedAt: number | null;
  /** When this conversation was last live: its newest turn, else when the chat
   *  was created (see `chatActiveAt`). */
  chatActiveAt: number;
  /** This chat was spawned in this app session, so its opening turn *is* the
   *  conversation — there is no "since we last spoke". */
  freshlySpawned: boolean;
  /** A digest already fired for this project in this app session. Once per
   *  session per project: the second one in a sitting reads as a bug. */
  alreadyAsked: boolean;
}

/** Should the tab dispatch a digest into the chat it just opened?
 *
 *  The guards are ordered cheapest-first, but each is independently sufficient —
 *  none of them is a heuristic standing in for another. */
export function shouldAskForStandup(s: StandupSignals): boolean {
  if (s.alreadyAsked) return false;
  if (s.freshlySpawned) return false;
  if (s.boardMovedAt === null) return false;
  // Strictly newer: a board whose last movement *is* the last thing this chat
  // did (the PM's own accepted proposal, a settle review it already answered)
  // has told it nothing new.
  return s.boardMovedAt > s.chatActiveAt;
}

/** When this conversation was last live, from the cheapest honest signals the
 *  chat carries.
 *
 *  The newest turn's end — or its start, for one still in flight — is the real
 *  answer, and it covers the settle review too: those arrive as ordinary
 *  user-role turns, so a chat that was handed a review a minute ago is correctly
 *  read as current. A chat with no turns at all falls back to when it was
 *  created, which is the last moment it can be said to have been in sync with
 *  the board.
 *
 *  Turns arrive in `seq` order, so the last element is the newest; a turn that
 *  never started (a failed send awaiting retry) carries no timestamp and is
 *  skipped rather than counted as now. */
export function chatActiveAt(chat: Pick<AgentRecord, "created_at">, turns: UserTurn[]): number {
  for (let i = turns.length - 1; i >= 0; i--) {
    const t = turns[i];
    const at = t.ended_at ?? t.started_at;
    if (at !== null && at !== undefined) return at;
  }
  // ISO string on the record (see the Rust `millis_to_iso`); an unparseable one
  // reads as epoch 0, which makes any board movement newer — the safe direction
  // for a chat we can't date.
  const created = Date.parse(chat.created_at);
  return Number.isNaN(created) ? 0 : created;
}
