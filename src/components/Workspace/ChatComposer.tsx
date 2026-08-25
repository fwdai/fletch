// The composer docked under a chat log, bound to one agent: the working strip
// that slides up from behind it, the model/effort controls, the mention and
// issue pickers, and the send/stop wiring.
//
// Extracted from ChatView so the run monitor's thread — one continuous log over
// several step agents — routes its single composer to whichever agent is live
// through the identical send machinery instead of a second copy of it.

import { useCallback, useEffect, useState } from "react";
import { type AgentRecord, api } from "@/api";
import { Composer } from "@/components/Composer";
import { providerLabel } from "@/data/providers";
import { getLinearTeamId } from "@/storage/projectSettings";
import { useAppStore } from "@/store";
import { ChatWorkingStatus } from "./ChatWorkingStatus";

export function ChatComposer({
  agent,
  activeModel,
  liveBusy,
  liveStartedAt,
  onSend,
}: {
  agent: AgentRecord;
  /** Model the agent actually used most recently (from its transcript). */
  activeModel: string | undefined;
  /** Debounced "is working" — drives the working strip above the composer. */
  liveBusy: boolean;
  /** Epoch millis the open turn started; when set, the strip's timer ticks. */
  liveStartedAt: number | undefined;
  /** Fired just before the message is dispatched — the owner of the scroll
   *  container re-pins its log to the bottom here. */
  onSend?: () => void;
}) {
  const transcriptLoading = useAppStore((s) => s.transcriptLoading[agent.id] ?? false);
  const busy = useAppStore((s) => s.managedBusy[agent.id] ?? false);
  const busyLabel = useAppStore((s) => s.managedBusyLabel[agent.id]);
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
  // inline arrow would re-fire it on every render (and double-append under
  // StrictMode's double-invoked effects).
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

  // Mid-turn follow-ups are allowed: a busy (running) agent still accepts a
  // message — delivered live (claude) or queued for the next turn boundary
  // (per-turn agents). So `canSend` no longer gates on `busy`; the Composer
  // shows Stop when empty and Send once the user types (see Composer).
  const canSend =
    !transcriptLoading &&
    !switchInFlight &&
    (agent.status === "running" || agent.status === "idle");

  return (
    <div className="composer-wrap">
      <div className="composer-stack">
        <div className="composer-anchor">
          <ChatWorkingStatus
            visible={liveBusy}
            label={busyLabel ?? `${customAgent?.name ?? providerLabel(agent.provider)} is working`}
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
            listIssues={repoPath ? () => api.listTrackerIssues(repoPath, linearTeamId) : undefined}
            listIssueComments={
              repoPath ? (issue) => api.issueComments(repoPath, issue.source, issue.key) : undefined
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
              onSend?.();
              send(agent.id, text, attachments);
            }}
            onStop={() => stop(agent.id)}
          />
        </div>
      </div>
    </div>
  );
}
