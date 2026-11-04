// Everything derived from an agent's chat log, in one place: the lazy history
// load, the display-policy + tool-pairing pass, and the per-turn bookkeeping
// (navigable prompts, closing footers, which turn is open).
//
// Extracted from ChatView so the native view's transcript rail renders the
// identical log from the identical derivation — the two surfaces differ only in
// chrome, and a second copy of this logic would drift.
import { useEffect, useMemo, useRef } from "react";
import { getAdapter } from "@/adapters";
import type { AgentRecord } from "@/api";
import { type ChatItem, useAppStore } from "@/store";
import { stripInjectedInstructions } from "@/util/instructions";
import type { ChatTurn } from "../ChatNav";
import { type PairCache, type ViewItem, visibleItems } from "./pair";
import {
  isPrompt,
  openTurnStart as openTurnStartOf,
  type TurnFooterData,
  turnFooters,
} from "./rows";
import { isUserInputTool } from "./UserInput/parse";

export interface Transcript {
  items: ViewItem[];
  turns: ChatTurn[];
  /** Parallel to `items`: the navigable-turn ordinal for user prompts. */
  turnIds: (number | undefined)[];
  /** Parallel to `items`: a footer at each *ended* turn's last row. */
  turnFooters: (TurnFooterData | null)[];
  /** Index where the currently-open turn begins. Only rows at or after it may
   *  show a live spinner, so a dangling tool_call from an interrupted earlier
   *  turn can't light up when a later turn goes busy. */
  openTurnStart: number;
  /** Persisted start of the open turn, for the live timer after a reload. */
  openTurnStartedAt: number | undefined;
  /** Model the agent actually used most recently; undefined for providers that
   *  don't report it, or before the first turn. */
  activeModel: string | undefined;
  /** The last row is an unanswered user-input widget — the agent is waiting on
   *  the user, not working, so the "is thinking" spinner must be suppressed. */
  awaitingInput: boolean;
  transcriptLoading: boolean;
  /** The agent has had a conversation of its own (distinguishes "empty new
   *  session" from "transcript missing on disk"). */
  hasPriorConversation: boolean;
  /** Raw log identity — a stable dep for scroll-to-bottom effects. */
  log: unknown;
}

/** An agent's own conversation: its log from the store, loaded from history
 *  on first sight (useHistoryLoad), derived for display (useTranscriptFrom). */
export function useTranscript(agent: AgentRecord): Transcript {
  const log = useAppStore((s) => s.managedLogs[agent.id]);
  const transcriptLoading = useAppStore((s) => s.transcriptLoading[agent.id] ?? false);
  useHistoryLoad(agent);
  return useTranscriptFrom(agent, log, transcriptLoading);
}

/** Kick off the lazy history load for an agent whose log is not in the store
 *  yet. Called by every surface that reads the log (the chat, the native rail,
 *  a sub-agent thread) so whichever mounts first after a reload gets it. */
export function useHistoryLoad(agent: AgentRecord): void {
  const log = useAppStore((s) => s.managedLogs[agent.id]);
  const transcriptLoading = useAppStore((s) => s.transcriptLoading[agent.id] ?? false);
  const transcriptLoaded = useAppStore((s) => s.transcriptLoaded[agent.id] ?? false);
  const switchInFlight = useAppStore((s) => s.switchInFlight[agent.id] ?? false);
  const loadHistoryTranscript = useAppStore((s) => s.loadHistoryTranscript);

  // A first message was sent (it becomes the task), so a provider session
  // holds a conversation of the agent's own.
  const hasPriorConversation = agent.task.trim().length > 0;
  // History to load: that own conversation, or the one the agent's session
  // continues through lineage (a fork) — there before its first message.
  const hasHistory = (Boolean(agent.session_id) && hasPriorConversation) || agent.lineage != null;

  useEffect(() => {
    if (!hasHistory || transcriptLoaded || transcriptLoading || switchInFlight) {
      return;
    }
    if (log !== undefined) {
      return;
    }
    void loadHistoryTranscript(agent.id);
  }, [
    agent.id,
    hasHistory,
    loadHistoryTranscript,
    log,
    switchInFlight,
    transcriptLoaded,
    transcriptLoading,
  ]);
}

/** Derive a display transcript from any log rendered as `agent`'s — its own,
 *  or one of its sub-agent threads (a tool_call's children). */
export function useTranscriptFrom(
  agent: Pick<AgentRecord, "provider" | "task">,
  log: ChatItem[] | undefined,
  transcriptLoading: boolean,
): Transcript {
  const hasPriorConversation = agent.task.trim().length > 0;

  // Persist tool_pair wrapper identity across renders so memoized rows survive
  // streaming deltas (see PairCache). Self-evicts stale ids each pass, so it
  // needs no reset when switching agents (tool-use ids never collide).
  const pairCache = useRef<PairCache>(new Map());

  const items = useMemo(
    () => visibleItems(log ?? [], getAdapter(agent.provider).policy, pairCache.current),
    [log, agent.provider],
  );

  // Navigable turns = the prompts the user wrote (git-action chips excluded,
  // `isPrompt`). Each gets a stable ordinal that maps an item to its
  // `data-chat-turn` marker, so ChatNav can jump straight to any bubble.
  const { turns, turnIds } = useMemo(() => {
    const turns: ChatTurn[] = [];
    const turnIds = items.map((it) => {
      if (!isPrompt(it)) return undefined;
      const id = turns.length;
      turns.push({ id, text: stripInjectedInstructions(it.text) });
      return id;
    });
    return { turns, turnIds };
  }, [items]);

  const { footers: footersOf, openStartedAt: openTurnStartedAt } = useMemo(
    () => turnFooters(items),
    [items],
  );

  const activeModel = useMemo(() => {
    for (let i = items.length - 1; i >= 0; i -= 1) {
      const it = items[i];
      if (it.kind === "agent_message" && it.model) return it.model;
    }
    return undefined;
  }, [items]);

  const awaitingInput = useMemo(() => {
    const last = items[items.length - 1];
    return Boolean(
      last && last.kind === "tool_pair" && isUserInputTool(last.call.name) && !last.result,
    );
  }, [items]);

  const openTurnStart = useMemo(() => openTurnStartOf(items), [items]);

  return {
    items,
    turns,
    turnIds,
    turnFooters: footersOf,
    openTurnStart,
    openTurnStartedAt,
    activeModel,
    awaitingInput,
    transcriptLoading,
    hasPriorConversation,
    log,
  };
}
