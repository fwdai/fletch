// The composer docked under a chat log, bound to one agent: the working strip
// that slides up from behind it, the model/effort controls, the mention and
// issue pickers, and the send/stop wiring.
//
// Extracted from ChatView so the run monitor's thread — one continuous log over
// several step agents — routes its single composer to whichever agent is live
// through the identical send machinery instead of a second copy of it.

import { useCallback, useEffect, useState } from "react";
import { type AgentRecord, api, type FolderGrant } from "@/api";
import { Composer } from "@/components/Composer";
import { providerLabel } from "@/data/providers";
import { spawnStageLabel } from "@/data/spawnStage";
import { isAgentBusy } from "@/helpers";
import { getLinearTeamId } from "@/storage/projectSettings";
import { useAppStore } from "@/store";
import { currentAccountId } from "./AccountPicker/choices";
import { ChatWorkingStatus } from "./ChatWorkingStatus";

/** The path the backend stored for a folder granted as `requested`: canonical,
 *  so it can differ from what was picked (a symlink, `~`). The grant just made
 *  is the one whose path `requested` resolves to; the requested path stands in
 *  when none matches exactly. */
function grantedPath(grants: FolderGrant[], requested: string): string {
  const trimmed = requested.replace(/\/+$/, "");
  return grants.find((g) => g.path === trimmed)?.path ?? (grants.at(-1)?.path || trimmed);
}

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
  /** "Is working" (see useLiveBusy) — drives the working strip above the composer. */
  liveBusy: boolean;
  /** Epoch millis the open turn started; when set, the strip's timer ticks. */
  liveStartedAt: number | undefined;
  /** Fired just before the message is dispatched — the owner of the scroll
   *  container re-pins its log to the bottom here. */
  onSend?: () => void;
}) {
  const transcriptLoading = useAppStore((s) => s.transcriptLoading[agent.id] ?? false);
  // Raw busy (a paused question still counts) for the Stop affordance; the
  // strip gets the question-aware `liveBusy` from the owner.
  const busy = useAppStore((s) => isAgentBusy(s, agent.id, agent.status));
  const busyLabel = useAppStore((s) => s.busyLabel[agent.id]);
  const switchInFlight = useAppStore((s) => s.switchInFlight[agent.id] ?? false);
  // Which step a fresh spawn is on, so the disabled composer says "Cloning
  // repository…" rather than only "Agent is not ready".
  const spawnStage = useAppStore((s) => s.spawnStage[agent.id]);
  const send = useAppStore((s) => s.sendUserMessage);
  const setAgentEffort = useAppStore((s) => s.setAgentEffort);
  const setAgentModel = useAppStore((s) => s.setAgentModel);
  const grantFolder = useAppStore((s) => s.grantAgentFolder);
  const revokeFolder = useAppStore((s) => s.revokeAgentFolder);
  const applyChangesNow = useAppStore((s) => s.applyAgentChangesNow);
  // A folder change the running turn holds back: offered "apply now".
  const folderChangeWaiting = useAppStore((s) => s.folderChangeWaiting[agent.id] === true);
  const switchAccount = useAppStore((s) => s.switchAgentAccount);
  const switchingAccount = useAppStore((s) => s.switchingAccount[agent.id] === true);
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
            notice={
              folderChangeWaiting
                ? {
                    text: "folder access changes when this turn ends",
                    action: "Apply now",
                    actionTip:
                      "Stop this turn so the agent restarts with the folder and gets your message now",
                    onAction: () => void applyChangesNow(agent.id),
                  }
                : undefined
            }
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
            account={{
              current: currentAccountId(agent),
              onPick: (id) => void switchAccount(agent.id, id),
              locked: busy || switchingAccount,
            }}
            disabled={!canSend}
            placeholder={
              canSend
                ? undefined
                : transcriptLoading
                  ? "Loading transcript…"
                  : switchInFlight
                    ? "Switching view…"
                    : (spawnStageLabel(spawnStage) ?? "Agent is not ready")
            }
            stopping={busy}
            mentionSource={() =>
              api.listCheckoutTree(agent.id).then((files) => files.map((f) => f.path))
            }
            listDir={api.listDir}
            listPrs={() => api.listPrs(agent.id)}
            folders
            grantedFolders={agent.folder_grants}
            onChangeGrantedFolder={(path, writable) => void grantFolder(agent.id, path, writable)}
            onRevokeFolder={(path) => void revokeFolder(agent.id, path)}
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
            onSend={async ({ text, attachments, folders }) => {
              // Folder chips are granted first, each queued with the agent's
              // config changes: the agent relaunches with the folder before
              // this message reaches it. Mid-turn, the message waits for the
              // turn's end (or "Apply now" in the working strip); one the
              // agent can already read goes in live. A refusal is in the error
              // banner, and returning false puts the draft back to fix.
              const granted: string[] = [];
              for (const folder of folders) {
                const grants = await grantFolder(agent.id, folder.path, folder.writable);
                if (!grants) return false;
                granted.push(grantedPath(grants, folder.path));
              }
              // Effort is session-level now (persisted via onChangeEffort and
              // read from the record each turn), so sends carry no per-message
              // effort — the composer's `thinking` in the payload is only used
              // by the new-agent spawn path.
              onSend?.();
              // The folders ride as attachments, named as folders to the
              // agent (`attachments::reference_line`).
              void send(agent.id, text, [...attachments, ...granted]);
              return true;
            }}
            onStop={() => stop(agent.id)}
          />
        </div>
      </div>
    </div>
  );
}
