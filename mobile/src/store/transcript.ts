// Records → chat items, through the desktop's own per-provider adapters, and
// the host store's reads of an agent's history that put them on screen
// (`createTranscriptSlice`).
//
// The desktop's `@/helpers/transcript` does the same thing, but its module also
// holds `applyEvent`, which is typed against the desktop `AppState` — importing
// it would pull the whole desktop store (and its component graph) into mobile's
// typecheck. The adapters themselves are what matter, and those are shared
// verbatim.

import type { AgentManagedEvent } from "@desktop/api/types/agent";
import type { LiveTurn, SessionRecord, UserTurn } from "@desktop/api/types/session";
import { mergeUserTurns, turnsOnPage } from "@desktop/helpers/mergeTurns";
import { hasSentTurn } from "@desktop/helpers/mirrorTurn";
import { newestWins } from "@desktop/util/newestWins";
import { type ChatItem, getAdapter, type RawEvent } from "../adapters";
import type { HostLink } from "../host";
import { isBusy } from "../lib/agents";
import { agentOf, type HostState } from "./host";
import { replayLiveTurn, runningTurnStart } from "./liveTurn";

type Set = (partial: Partial<HostState> | ((s: HostState) => Partial<HostState>)) => void;
type Get = () => HostState;

/** The part of an agent's history the phone holds: every page loaded so far,
 *  concatenated oldest first, and the cursor for the page before them (null
 *  once there is none, or on a host that served the history whole). Kept as
 *  records rather than items because a page can start mid-turn: a tool result
 *  whose call is on the page before only pairs with it once both are reduced
 *  together. */
export interface LoadedHistory {
  records: SessionRecord[];
  older: string | null;
}

/** How the last read of an agent's history went. `ready` with nothing to show
 *  is not an empty chat — a chat is created by its first message — so the
 *  screens treat it as a history the host could not produce. */
export type LogLoad =
  | { status: "loading" }
  | { status: "ready" }
  | { status: "error"; error: string };

/** Render canonical session records exactly as on-disk replay does:
 *  `normalizeTranscript` → `reduce`, each item carrying the `recordSeq` and
 *  `recordSession` of the record it came from. Adapter throws degrade to a
 *  partial log rather than an empty screen. */
export function reduceRecords(provider: string | undefined, records: SessionRecord[]): ChatItem[] {
  const adapter = getAdapter(provider);
  let raw: RawEvent[];
  try {
    raw = adapter.normalizeTranscript(
      records.map((r) => r.body),
      records.map((r) => r.seq),
      records.map((r) => r.session_id),
    );
  } catch {
    return [];
  }
  let items: ChatItem[] = [];
  for (const ev of raw) {
    try {
      items = adapter.reduce(items, ev);
    } catch {
      // Skip the one bad event; keep the rest of the transcript.
    }
  }
  return items;
}

/** The log a page of history draws: its records, with the agent's turns merged
 *  in by position (`mergeUserTurns`) — those the page can place, so a turn on
 *  an older page doesn't land at the top of this one. */
export function renderHistory(
  provider: string | undefined,
  records: SessionRecord[],
  turns: UserTurn[],
): ChatItem[] {
  return mergeUserTurns(reduceRecords(provider, records), turnsOnPage(turns, records));
}

/** What a log holds past the history it was rendered from: the replayed
 *  running turn, live frames, a send not yet in the records. Every item
 *  rendered from a record carries its seq and nothing after the history does,
 *  so it is whatever follows the last stamped item — less the turn bubbles a
 *  re-render of the history (`fresh`) draws itself. */
export function pastHistory(log: ChatItem[], fresh: ChatItem[]): ChatItem[] {
  let end = log.length;
  while (end > 0 && log[end - 1].recordSeq === undefined) end -= 1;
  return log.slice(end).filter((it) => {
    const turnId =
      it.kind === "user_message" || it.kind === "queued_message" ? it.turnId : undefined;
    return turnId === undefined || !hasSentTurn(fresh, turnId);
  });
}

/** Apply one live event to an agent's log. Returns the next log plus whether
 *  the turn ended (any adapter's `turn_end` notice), mirroring the desktop's
 *  `applyEvent` contract. */
export function applyLiveEvent(
  provider: string | undefined,
  prev: ChatItem[],
  rawEvent: RawEvent,
): { items: ChatItem[]; turnEnded: boolean } {
  const adapter = getAdapter(provider);
  let next: ChatItem[];
  try {
    next = adapter.reduce(prev, rawEvent);
  } catch {
    return { items: prev, turnEnded: false };
  }
  if (next === prev) return { items: prev, turnEnded: false };
  const last = next[next.length - 1];
  const turnEnded =
    next.length > prev.length && last?.kind === "notice" && last.subtype === "turn_end";
  return { items: next, turnEnded };
}

export interface TranscriptSlice {
  logs: Record<string, ChatItem[]>;
  /** Per agent, how the last read of its history from the host went — what
   *  tells a chat that is still loading (or failed to) apart from one that
   *  loaded. Absent until a read has been asked for. */
  logLoads: Record<string, LogLoad>;
  /** Per agent, the records its log was last reduced from and the cursor for
   *  older ones (`LoadedHistory`). What `loadOlderLog` extends; a set `older`
   *  is what offers the chat's "Load older messages". */
  histories: Record<string, LoadedHistory>;
  loadAgent(agentId: string): Promise<void>;
  /** Rebuild the log from the host's records. With `liveTurn`, the running
   *  turn is replayed on top from `read_live_turn` (see store/liveTurn).
   *  Reports how the read went in `logLoads`, not `lastError`: the chat that
   *  needed it is where the failure is shown. */
  rebuildLog(agentId: string, opts?: { liveTurn?: boolean }): Promise<void>;
  /** Read the page before the oldest one loaded and put it at the head of the
   *  log. A no-op when there is no older page to read. A failure rejects and
   *  is not put in `lastError`: the control that asked shows it. */
  loadOlderLog(agentId: string): Promise<void>;
}

/** `link` is the link of the host the slice acts on. */
export function createTranscriptSlice(set: Set, get: Get, link: HostLink): TranscriptSlice {
  const { client, api } = link;

  /** Per agent, the order of its history reads: opening a chat, a reconnect
   *  and a turn end can each start one, and only the newest says how loading
   *  went. */
  const logReads = new Map<string, ReturnType<typeof newestWins>>();
  const claimLogRead = (agentId: string) => {
    let order = logReads.get(agentId);
    if (!order) {
      order = newestWins();
      logReads.set(agentId, order);
    }
    return order.claim();
  };

  return {
    logs: {},
    logLoads: {},
    histories: {},

    async loadAgent(agentId) {
      // The host's records stop at the last finished turn (the running one is
      // ingested when it ends), so a busy agent's log is rebuilt from the records
      // plus the running turn as the host has it (`read_live_turn`) — whatever
      // the socket missed while the app was in the background, for any provider.
      // A host without the op leaves the log the live events built rather than
      // replacing it with the prompt alone; the turn-end `session:records-
      // appended` rebuild is the authoritative one there.
      const agent = agentOf(get(), agentId);
      const busy = agent !== undefined && isBusy(agent);
      if (busy && get().hostSupports("read_live_turn")) {
        await get().rebuildLog(agentId, { liveTurn: true });
      } else if (!busy || get().logs[agentId] === undefined) {
        await get().rebuildLog(agentId);
      }
      // A purpose-tagged chat has no code of its own to report on — the PM never
      // edits a file, and the host denies it the publish ops — so the git and PR
      // reads are skipped rather than answered with an empty diff.
      if (!agent?.purpose) await get().loadGit(agentId);
    },

    async rebuildLog(agentId, opts = {}) {
      const read = claimLogRead(agentId);
      const report = (load: LogLoad) => {
        if (read.current()) set((s) => ({ logLoads: { ...s.logLoads, [agentId]: load } }));
      };
      // Everything a read writes lands in one update, and only while it is the
      // newest: an older read resolving last would otherwise put its transcript
      // and cursor over the newer one's, under the newer one's `ready`.
      const commit = (patch: (s: HostState) => Partial<HostState>) => {
        if (!read.current()) return;
        set((s) => ({
          ...patch(s),
          logLoads: { ...s.logLoads, [agentId]: { status: "ready" } },
        }));
      };
      const rebuild = async () => {
        // Only the newest page, where the host has pages: a long session's
        // history runs to tens of MB of tool results, and the phone needs the
        // last turns to paint. Every rebuild starts over from it, the turn-end
        // one included — so older pages the user had loaded are dropped then
        // and read again on demand. Simpler than splicing the new records onto
        // pages whose cursors the turn may have outdated, and cheap: it is one
        // page either way.
        const readNewest = async (): Promise<LoadedHistory> =>
          get().hostSupports("read_session_page")
            ? api.readSessionPage(agentId)
            : { records: await api.readSessionRecords(agentId), older: null };
        let [history, turns] = await Promise.all([readNewest(), api.readUserTurns(agentId)]);
        // The host ingests a turn's transcript into session_records only at
        // turn-end, and that ingest can insert nothing (session id not captured
        // yet, transcript not located). Mirror the desktop's readHistory: with no
        // records of the agent's own (a fork may still show inherited ones), ask
        // for a backfill and read again before concluding there is no history.
        // The newest page answers that as well as the whole list: an agent's own
        // records are the newest in its history.
        if (history.records.every((r) => r.inherited)) {
          await api.syncSession(agentId);
          [history, turns] = await Promise.all([readNewest(), api.readUserTurns(agentId)]);
        }
        const { records } = history;
        const histories = (s: HostState) => ({ ...s.histories, [agentId]: history });
        // Still nothing stored and no running turn to replay: keep the log the
        // live events built rather than wiping the conversation the user was
        // just looking at (the desktop's records-appended handler makes the same
        // call). With no log either, the chat says the history is missing —
        // never that there is no conversation (see `LogLoad`).
        if (records.length === 0 && !opts.liveTurn) {
          commit((s) => ({ histories: histories(s) }));
          return;
        }
        const provider = agentOf(get(), agentId)?.provider;
        const items = renderHistory(provider, records, turns);
        if (!opts.liveTurn) {
          commit((s) => ({ histories: histories(s), logs: { ...s.logs, [agentId]: items } }));
          return;
        }
        // The running turn is read last, once the records have settled, so the
        // stream has the shortest possible window to move on in — and every
        // frame that lands in that window is kept, to be folded after the
        // snapshot or skipped as already in it (see replayLiveTurn). The patch
        // below runs in the response's own continuation, before the next frame
        // is dispatched, so nothing arrives between the capture and the fold.
        const late: AgentManagedEvent[] = [];
        const off = client.on("agent:event", (payload) => {
          const e = payload as AgentManagedEvent;
          if (e.agent_id === agentId) late.push(e);
        });
        let live: LiveTurn;
        try {
          live = await api.readLiveTurn(agentId);
        } finally {
          off();
        }
        const startedAt = runningTurnStart(turns);
        commit((s) => ({
          histories: histories(s),
          ...replayLiveTurn(s, agentId, items, live, late),
          ...(startedAt === undefined
            ? {}
            : { turnStartedAt: { ...s.turnStartedAt, [agentId]: startedAt } }),
        }));
      };
      report({ status: "loading" });
      try {
        await rebuild();
      } catch (e) {
        report({ status: "error", error: e instanceof Error ? e.message : String(e) });
        throw e;
      }
    },

    async loadOlderLog(agentId) {
      const loaded = get().histories[agentId];
      if (!loaded?.older) return;
      const [page, turns] = await Promise.all([
        api.readSessionPage(agentId, loaded.older),
        api.readUserTurns(agentId),
      ]);
      // A rebuild that landed meanwhile replaced the pages this one extends;
      // its log is the newer, and the cursor it holds is the one to follow.
      if (get().histories[agentId] !== loaded) return;
      const provider = agentOf(get(), agentId)?.provider;
      const records = [...page.records, ...loaded.records];
      // Re-reduced whole rather than prepended, so a call on this page pairs
      // with its result on the next. Everything the log holds past the loaded
      // records (a replayed running turn, live frames, a queued send) is not in
      // them and is carried over as it is.
      const items = renderHistory(provider, records, turns);
      set((s) => ({
        histories: { ...s.histories, [agentId]: { records, older: page.older } },
        logs: { ...s.logs, [agentId]: [...items, ...pastHistory(s.logs[agentId] ?? [], items)] },
      }));
    },
  };
}
