// Everything derived from an agent's chat log, in one place: the lazy history
// load, the display-policy + tool-pairing pass, and the per-turn bookkeeping
// (navigable prompts, closing footers, which turn is open).
//
// Extracted from ChatView so the native view's transcript rail renders the
// identical log from the identical derivation — the two surfaces differ only in
// chrome, and a second copy of this logic would drift.
import { useEffect, useMemo, useRef } from "react";
import { applyPolicy, getAdapter } from "@/adapters";
import type { AgentRecord } from "@/api";
import { APP_ACTION_PREFIX } from "@/delegation";
import { useAppStore } from "@/store";
import { stripInjectedInstructions } from "@/util/instructions";
import type { ChatTurn } from "../ChatNav";
import { type PairCache, pairToolItems, type ViewItem } from "./pair";
import { isUserInputTool } from "./UserInput/parse";

/** A turn's closing footer: how long it ran, its settled prose (for copy), and
 *  its ordinal among navigable prompts (the fork cutoff). */
export interface TurnFooterData {
  runSec: number;
  copyText: string;
  turnOrdinal: number;
}

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
  /** The agent has history worth loading (distinguishes "empty new session"
   *  from "transcript missing on disk"). */
  hasPriorConversation: boolean;
  /** Raw log identity — a stable dep for scroll-to-bottom effects. */
  log: unknown;
}

export function useTranscript(agent: AgentRecord): Transcript {
  const log = useAppStore((s) => s.managedLogs[agent.id]);
  const transcriptLoading = useAppStore((s) => s.transcriptLoading[agent.id] ?? false);
  const transcriptLoaded = useAppStore((s) => s.transcriptLoaded[agent.id] ?? false);
  const switchInFlight = useAppStore((s) => s.switchInFlight[agent.id] ?? false);
  const loadHistoryTranscript = useAppStore((s) => s.loadHistoryTranscript);

  const hasSession = Boolean(agent.session_id);
  const hasPriorConversation = agent.task.trim().length > 0;

  useEffect(() => {
    if (!hasSession || transcriptLoaded || transcriptLoading || switchInFlight) {
      return;
    }
    if (log !== undefined || !hasPriorConversation) {
      return;
    }
    void loadHistoryTranscript(agent.id);
  }, [
    agent.id,
    hasSession,
    hasPriorConversation,
    loadHistoryTranscript,
    log,
    switchInFlight,
    transcriptLoaded,
    transcriptLoading,
  ]);

  // Persist tool_pair wrapper identity across renders so memoized rows survive
  // streaming deltas (see PairCache). Self-evicts stale ids each pass, so it
  // needs no reset when switching agents (tool-use ids never collide).
  const pairCache = useRef<PairCache>(new Map());

  const items = useMemo(() => {
    const adapter = getAdapter(agent.provider);
    const visible = applyPolicy(log ?? [], adapter.policy);
    return pairToolItems(visible, pairCache.current);
  }, [log, agent.provider]);

  // Navigable turns = the real user prompts (git-action chips excluded). Each
  // gets a stable ordinal that maps an item to its `data-chat-turn` marker, so
  // ChatNav can jump straight to any bubble.
  const { turns, turnIds } = useMemo(() => {
    const turns: ChatTurn[] = [];
    const turnIds = items.map((it) => {
      if (it.kind !== "user_message" || it.text.startsWith(APP_ACTION_PREFIX)) {
        return undefined;
      }
      const id = turns.length;
      turns.push({ id, text: stripInjectedInstructions(it.text) });
      return id;
    });
    return { turns, turnIds };
  }, [items]);

  // Footer closing each *ended* turn (border + "Ran …" + copy), placed at the
  // turn's last item — the seam before the next turn. Gated on the same
  // turn-end signal as the duration, so it only appears once the turn finishes;
  // the open turn (started, not ended) carries no footer, just its live timer
  // on the working strip.
  const { turnFooters, openTurnStartedAt } = useMemo(() => {
    const footers: (TurnFooterData | null)[] = items.map(() => null);
    let openStart: number | undefined;
    const starts: number[] = [];
    items.forEach((it, i) => {
      if (it.kind === "user_message" && !it.text.startsWith(APP_ACTION_PREFIX)) starts.push(i);
    });
    starts.forEach((startIdx, k) => {
      const start = items[startIdx];
      if (start.kind !== "user_message" || start.startedAt == null) return;
      const endExclusive = k + 1 < starts.length ? starts[k + 1] : items.length;
      if (start.endedAt == null) {
        openStart = start.startedAt; // turn still running
        return;
      }
      // The agent's settled prose for this turn — what "copy" yields.
      const texts: string[] = [];
      for (let j = startIdx; j < endExclusive; j += 1) {
        const it = items[j];
        if (it.kind === "agent_message" && !it.streaming && it.text) texts.push(it.text);
      }
      footers[endExclusive - 1] = {
        runSec: (start.endedAt - start.startedAt) / 1000,
        copyText: texts.join("\n\n"),
        turnOrdinal: k,
      };
    });
    return { turnFooters: footers, openTurnStartedAt: openStart };
  }, [items]);

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

  const openTurnStart = useMemo(() => {
    for (let i = items.length - 1; i >= 0; i -= 1) {
      if (items[i].kind === "user_message") return i;
    }
    return 0;
  }, [items]);

  return {
    items,
    turns,
    turnIds,
    turnFooters,
    openTurnStart,
    openTurnStartedAt,
    activeModel,
    awaitingInput,
    transcriptLoading,
    hasPriorConversation,
    log,
  };
}
