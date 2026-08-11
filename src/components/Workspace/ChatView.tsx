import { useCallback, useEffect, useRef, useState } from "react";
import { type AgentRecord, api } from "@/api";
import { Composer } from "@/components/Composer";
import { providerLabel } from "@/data/providers";
import { getLinearTeamId } from "@/storage/projectSettings";
import { useAppStore } from "@/store";
import { ChatSearch } from "./ChatSearch";
import { ChatWorkingStatus } from "./ChatWorkingStatus";
import { TranscriptList } from "./messages/TranscriptList";
import { isTurnPending } from "./messages/turnPending";
import { useTranscript } from "./messages/useTranscript";
import { useLiveBusy } from "./useLiveBusy";

/** Custom-view body: scrolling chat log + composer at the bottom.
 *  The composer here dispatches the user's message via the store; it
 *  doesn't care about provider routing yet. */
export function ChatView({ agent }: { agent: AgentRecord }) {
  const transcriptLoading = useAppStore((s) => s.transcriptLoading[agent.id] ?? false);
  const busy = useAppStore((s) => s.managedBusy[agent.id] ?? false);
  const busyLabel = useAppStore((s) => s.managedBusyLabel[agent.id]);
  const turnStartedAt = useAppStore((s) => s.turnStartedAt[agent.id]);
  const switchInFlight = useAppStore((s) => s.switchInFlight[agent.id] ?? false);
  const send = useAppStore((s) => s.sendUserMessage);
  const setAgentEffort = useAppStore((s) => s.setAgentEffort);
  const setAgentModel = useAppStore((s) => s.setAgentModel);
  const stop = useAppStore((s) => s.stop);
  const runLocalCommand = useAppStore((s) => s.runLocalCommand);
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

  const scrollRef = useRef<HTMLDivElement | null>(null);
  // Owned here so sending a message can re-pin the log to the bottom.
  const pinnedToBottom = useRef(true);
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

  // Log derivation (lazy history load, display policy, tool pairing, per-turn
  // bookkeeping) is shared with the native view's rail — see useTranscript.
  const transcript = useTranscript(agent);
  const { items, turns, activeModel, awaitingInput, openTurnStartedAt } = transcript;

  // Debounced "is working" (see useLiveBusy), shared with the Roadmap tab's PM
  // chat so both surfaces settle on the same beat.
  const liveBusy = useLiveBusy(agent.id, awaitingInput);

  // Phase A: user just sent a turn-starting prompt and nothing has landed yet.
  // A quiet inline anchor (dots only — label lives in the bottom status strip).
  // Only on the first turn: for later turns the chat already has content above
  // and the bottom status strip carries the "is working" signal, so the inline
  // anchor is redundant.
  const turnPending = liveBusy && isTurnPending(items) && turns.length <= 1;

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
      <TranscriptList
        agent={agent}
        transcript={transcript}
        liveBusy={liveBusy}
        pending={turnPending}
        scrollRef={scrollRef}
        pinRef={pinnedToBottom}
        hideNav={searchOpen}
      />
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
              listIssueComments={
                repoPath
                  ? (issue) => api.issueComments(repoPath, issue.source, issue.key)
                  : undefined
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
