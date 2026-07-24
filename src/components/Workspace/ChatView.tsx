import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { applyPolicy, getAdapter } from "@/adapters";
import { type AgentRecord, api } from "@/api";
import { Composer } from "@/components/Composer";
import { APP_ACTION_PREFIX } from "@/components/RightPanel/delegation";
import { Loader } from "@/components/ui/Loader";
import { providerLabel } from "@/data/providers";
import { getLinearTeamId } from "@/storage/projectSettings";
import { useAppStore } from "@/store";
import { stripInjectedInstructions } from "@/util/instructions";
import { ChatNav } from "./ChatNav";
import { ChatSearch } from "./ChatSearch";
import { ChatWorkingStatus } from "./ChatWorkingStatus";
import { MessageItem } from "./messages/MessageItem";
import { type PairCache, pairToolItems, rowKey } from "./messages/pair";
import { isTurnPending } from "./messages/turnPending";
import { isUserInputTool } from "./messages/UserInput/parse";
import { TurnFooter } from "./RunTimer";

/** Custom-view body: scrolling chat log + composer at the bottom.
 *  The composer here dispatches the user's message via the store; it
 *  doesn't care about provider routing yet. */
export function ChatView({ agent }: { agent: AgentRecord }) {
  const log = useAppStore((s) => s.managedLogs[agent.id]);
  const transcriptLoading = useAppStore((s) => s.transcriptLoading[agent.id] ?? false);
  const transcriptLoaded = useAppStore((s) => s.transcriptLoaded[agent.id] ?? false);
  const busy = useAppStore((s) => s.managedBusy[agent.id] ?? false);
  const busyLabel = useAppStore((s) => s.managedBusyLabel[agent.id]);
  const turnStartedAt = useAppStore((s) => s.turnStartedAt[agent.id]);
  const switchInFlight = useAppStore((s) => s.switchInFlight[agent.id] ?? false);
  const send = useAppStore((s) => s.sendUserMessage);
  const setAgentEffort = useAppStore((s) => s.setAgentEffort);
  const setAgentModel = useAppStore((s) => s.setAgentModel);
  const stop = useAppStore((s) => s.stop);
  const runLocalCommand = useAppStore((s) => s.runLocalCommand);
  const loadHistoryTranscript = useAppStore((s) => s.loadHistoryTranscript);
  const usage = useAppStore((s) => s.usage[agent.id]);
  // The custom agent this session was spawned from (if any, and still present),
  // so the chat surfaces the agent's name rather than its base provider.
  const customAgent = useAppStore((s) =>
    agent.custom_agent_id ? s.customAgents.find((a) => a.id === agent.custom_agent_id) : undefined,
  );
  const composerSeed = useAppStore((s) => s.composerSeeds[agent.id]);
  const consumeComposerSeed = useAppStore((s) => s.consumeComposerSeed);
  // Stable identity: the Composer's seed effect lists this in its deps, so an
  // inline arrow would re-fire it on every ChatView render (and double-append
  // under StrictMode's double-invoked effects).
  const onSeedConsumed = useCallback(
    () => consumeComposerSeed(agent.id),
    [agent.id, consumeComposerSeed],
  );

  // The project's configured Linear team, scoping the composer's issue
  // picker to the agent's primary repo. Undefined while loading or unset —
  // the picker then serves GitHub issues only.
  const repoPath = agent.repos[0]?.repo_path;
  const projectId = useAppStore((s) =>
    repoPath ? (s.workspace?.projects.find((p) => p.path === repoPath)?.project_id ?? "") : "",
  );
  const [linearTeamId, setLinearTeamId] = useState<string | undefined>();
  useEffect(() => {
    let cancelled = false;
    setLinearTeamId(undefined);
    getLinearTeamId(projectId)
      .then((teamId) => {
        if (!cancelled) setLinearTeamId(teamId);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");

  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    setSearchQuery("");
  }, []);

  // ⌘F / Ctrl+F opens find-in-conversation. A repeat press while open just
  // refocuses + selects the existing input (the bar is already mounted), which
  // mirrors how browsers behave.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "f" || e.key === "F")) {
        // The right-panel terminal has its own ⌘F (handled by xterm); its
        // keydown still bubbles to window, so ignore presses originating there.
        if ((e.target as HTMLElement | null)?.closest(".term-panel")) return;
        e.preventDefault();
        setSearchOpen(true);
        requestAnimationFrame(() => {
          const el = document.getElementById("chat-search-input") as HTMLInputElement | null;
          el?.focus();
          el?.select();
        });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Close the find bar when switching conversations — its matches belong to the
  // log we're leaving.
  useEffect(() => {
    setSearchOpen(false);
    setSearchQuery("");
  }, [agent.id]);

  // Whether the chat is "pinned" to the bottom. While true we follow new
  // messages; once the user scrolls up we stop auto-scrolling and leave their
  // position alone until they scroll back down to the bottom.
  const pinnedToBottom = useRef(true);
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

  // Re-pin to the bottom whenever we switch to a different agent, so each
  // conversation opens scrolled to its latest message.
  useEffect(() => {
    pinnedToBottom.current = true;
  }, [agent.id]);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    // Allow a small slop so the user counts as "at the bottom" even a few
    // pixels short — sub-pixel rounding otherwise makes exact equality flaky.
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    pinnedToBottom.current = distanceFromBottom <= 40;
  };

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (transcriptLoading) return;
    if (!pinnedToBottom.current) return;
    el.scrollTop = el.scrollHeight;
  }, [log, transcriptLoading]);

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
    const turns: { id: number; text: string }[] = [];
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
    // `turnOrdinal` is the footer turn's index among navigable prompts (the same
    // 0-based ordinal `turns` uses), which is exactly what the fork action needs
    // as its "up to this message" cutoff.
    const footers: ({ runSec: number; copyText: string; turnOrdinal: number } | null)[] = items.map(
      () => null,
    );
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

  // The model the agent actually used on its most recent turn (Claude, pi,
  // Codex, OpenCode report it in their transcripts). Undefined for Cursor /
  // Antigravity, or before the first turn — the composer then shows just the
  // provider.
  const activeModel = useMemo(() => {
    for (let i = items.length - 1; i >= 0; i -= 1) {
      const it = items[i];
      if (it.kind === "agent_message" && it.model) return it.model;
    }
    return undefined;
  }, [items]);
  // When the last row is an unanswered user-input widget, the agent is paused
  // on that tool waiting for the user — not working — so suppress the
  // "is thinking" spinner (the widget itself signals it's the user's turn).
  const awaitingInput = useMemo(() => {
    const last = items[items.length - 1];
    return Boolean(
      last && last.kind === "tool_pair" && isUserInputTool(last.call.name) && !last.result,
    );
  }, [items]);

  // The backend emits a transient `idle` between process spawn and the first
  // turn's `running` (every process rests at Idle at spawn). Raw `busy` thus
  // dips false→true mid-startup, which would flash the working strip and
  // restart the timer. Hold "working" through brief dips: rise immediately,
  // fall only after a short grace period.
  const liveBusyRaw = busy && !awaitingInput;
  const [liveBusy, setLiveBusy] = useState(liveBusyRaw);
  useEffect(() => {
    if (liveBusyRaw) {
      setLiveBusy(true);
      return;
    }
    const t = window.setTimeout(() => setLiveBusy(false), 700);
    return () => window.clearTimeout(t);
  }, [liveBusyRaw]);

  // Phase A: user just sent a turn-starting prompt and nothing has landed yet.
  // A quiet inline anchor (dots only — label lives in the bottom status strip).
  // Only on the first turn: for later turns the chat already has content above
  // and the bottom status strip carries the "is working" signal, so the inline
  // anchor is redundant.
  const turnPending = liveBusy && isTurnPending(items) && turns.length <= 1;

  // Index where the currently-open turn begins (the last top-level user
  // message). Only tools at/after it belong to the running turn, so only they
  // may show a live spinner — a dangling tool_call left by an interrupted or
  // reloaded earlier turn must not light up when a later turn sets `busy`.
  const openTurnStart = useMemo(() => {
    for (let i = items.length - 1; i >= 0; i -= 1) {
      if (items[i].kind === "user_message") return i;
    }
    return 0;
  }, [items]);

  // Live-timer anchor: the backend's turn-start timestamp (from `turn:started`,
  // the same value the footer's duration uses, so they never drift). On reload
  // mid-turn no event fired this session, so fall back to the open turn's
  // persisted start. Absent during spawn → strip shows, timer waits.
  const liveStartedAt = liveBusy ? (turnStartedAt ?? openTurnStartedAt) : undefined;

  // Mid-turn follow-ups are allowed: a busy (running) agent still accepts a
  // message — delivered live (claude) or queued for the next turn boundary
  // (per-turn agents). So `canSend` no longer gates on `busy`; the Composer
  // shows Stop when empty and Send once the user types (see Composer).
  const canSend =
    !transcriptLoading &&
    !switchInFlight &&
    (agent.status === "running" || agent.status === "idle");

  return (
    <div className="chat">
      {searchOpen && (
        <ChatSearch
          containerRef={scrollRef}
          query={searchQuery}
          onQueryChange={setSearchQuery}
          contentVersion={items}
          onClose={closeSearch}
        />
      )}
      <div className="chat-scroll-wrap">
        <div className="chat-scroll" ref={scrollRef} onScroll={handleScroll}>
          <div className="chat-inner fade-in" key={agent.id}>
            {transcriptLoading && items.length === 0 ? (
              <div className="writing flex-center">
                <Loader variant="accent" />
                <span>Loading transcript…</span>
              </div>
            ) : items.length === 0 && hasPriorConversation && !busy ? (
              <div className="empty-msg" style={{ margin: "40px auto", maxWidth: 360 }}>
                <div className="et">No transcript available</div>
                <div>
                  {providerLabel(agent.provider)}'s session file is not on disk for this agent.
                </div>
              </div>
            ) : (
              items.map((item, i) => (
                <Fragment key={rowKey(item, i)}>
                  <MessageItem
                    item={item}
                    provider={agent.provider}
                    agentId={agent.id}
                    busy={liveBusy && i >= openTurnStart}
                    turnId={turnIds[i]}
                  />
                  {turnFooters[i] != null && <TurnFooter {...turnFooters[i]!} agentId={agent.id} />}
                </Fragment>
              ))
            )}
            {turnPending && (
              <div className="chat-pending" aria-hidden="true">
                <Loader variant="muted" size="md" />
              </div>
            )}
          </div>
        </div>
        {!searchOpen && <ChatNav scrollRef={scrollRef} turns={turns} />}
      </div>
      <div className="composer-wrap">
        <div className="composer-stack">
          <div className="composer-anchor">
            <ChatWorkingStatus
              visible={liveBusy}
              label={
                busyLabel ?? `${customAgent?.name ?? providerLabel(agent.provider)} is working`
              }
              startedAt={liveStartedAt}
            />
            <Composer
              existingSession
              activeModel={activeModel}
              usage={usage}
              defaultProvider={agent.provider}
              projectDir={agent.repos[0]?.repo_path}
              onLocalCommand={(action) => runLocalCommand(action, agent.id)}
              defaultModel={agent.model ?? undefined}
              defaultCustomAgentId={agent.custom_agent_id ?? undefined}
              initialThinking={agent.effort ?? undefined}
              onChangeEffort={(value) => {
                // Go through the store so the change is serialized per agent and
                // a subsequent send waits for it to land (see queueConfigOp).
                // claude restarts to re-apply --effort; per-turn agents read the
                // new value from the record on their next turn.
                setAgentEffort(agent.id, value).catch((e) => {
                  console.error("set_agent_effort failed", e);
                });
              }}
              onChangeModel={(model) => {
                setAgentModel(agent.id, model ?? null).catch((e) => {
                  console.error("set_agent_model failed", e);
                });
              }}
              disabled={!canSend}
              placeholder={
                canSend
                  ? undefined
                  : transcriptLoading
                    ? "Loading transcript…"
                    : switchInFlight
                      ? "Switching view…"
                      : "Agent is not ready"
              }
              stopping={busy}
              mentionSource={() =>
                api.listCheckoutTree(agent.id).then((files) => files.map((f) => f.path))
              }
              listDir={api.listDir}
              listPrs={() => api.listPrs(agent.id)}
              listIssues={
                repoPath ? () => api.listTrackerIssues(repoPath, linearTeamId) : undefined
              }
              onPickIssue={(issue) => {
                // Persist the pick so the agent's eventual PR closes this
                // issue — the brief insert alone wouldn't survive to the
                // trailer. Best-effort: a failure only loses the trailer.
                api.setAgentIssueRef(agent.id, issue.key).catch((e) => {
                  console.error("set_agent_issue_ref failed", e);
                });
              }}
              seed={composerSeed}
              onSeedConsumed={onSeedConsumed}
              draftKey={agent.id}
              onSend={({ text, attachments }) => {
                // Effort is session-level now (persisted via onChangeEffort and
                // read from the record each turn), so sends carry no per-message
                // effort — the composer's `thinking` in the payload is only used
                // by the new-agent spawn path.
                pinnedToBottom.current = true;
                send(agent.id, text, attachments);
              }}
              onStop={() => stop(agent.id)}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
