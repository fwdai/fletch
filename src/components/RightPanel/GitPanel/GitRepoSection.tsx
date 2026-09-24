import { useEffect, useState } from "react";
import type { AgentRecord, TrackedRepo } from "@/api";
import { delegationLabel } from "@/delegation";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
import { ActionBar } from "./ActionBar";
import { AutopilotHistory, AutopilotSwitch } from "./Autopilot";
import { ChangesList } from "./ChangesList";
import { CommitComposer, CommitStatus } from "./CommitComposer";
import { BlockedConfigCard, ClosedPRCard, ConflictCard, PRCard } from "./cards";
import { EmptyState } from "./EmptyState";
import { FileDiffView } from "./FileDiffView";
import { useActionBarModel } from "./hooks/useActionBarModel";
import { useCommitDraft } from "./hooks/useCommitDraft";
import { useGitActions } from "./hooks/useGitActions";
import { useGitPanelData } from "./hooks/useGitPanelData";
import { usePrHistory } from "./hooks/usePrHistory";
import { useTransientFeedback } from "./hooks/useTransientFeedback";
import { PrSetStrip } from "./PrSetStrip";
import { StatusHeader } from "./StatusHeader";

/** One repo's worth of git panel: the full header / body / footer stack,
 *  scoped to a single checkout of the agent. For a single-repo agent this IS
 *  the whole panel. `subdir` is undefined for the agent's primary repo (index
 *  0) — that section reads/writes the plain agent-keyed store entries that
 *  live events and bulk polls update — and the checkout's directory name for
 *  secondaries, which read/write under `checkoutKey(agentId, subdir)`.
 *
 *  The component is a thin orchestrator: live reads + polling live in
 *  `useGitPanelData`, the commit draft in `useCommitDraft`, busy/notice in
 *  `useTransientFeedback`, the dispatch table in `useGitActions`, and the
 *  split-button model in `useActionBarModel`. The agent-handoff lifecycle is
 *  NOT here — it belongs to `useDelegationSync` at the app root, so a
 *  delegation completes whether or not this section is on screen; the section
 *  only reads the delegation to render it. */
export function GitRepoSection({
  agent,
  repo,
  subdir,
  autopilotSwitch = true,
}: {
  agent: AgentRecord;
  repo: TrackedRepo | undefined;
  subdir?: string;
  /** Whether this section's header carries the workspace's autopilot switch.
   *  It is per agent, not per checkout, so a multi-repo panel shows it once. */
  autopilotSwitch?: boolean;
}) {
  const {
    gitState,
    prState,
    checks,
    comments,
    mergeState,
    prOpen,
    panelState,
    fetchGitState,
    fetchPrState,
  } = useGitPanelData(agent.id, repo, subdir);

  const { busy, runBusy, notice, showNotice } = useTransientFeedback(agent.id);
  const { override, msg, setMsg, commitRef, customActive, openOverride, revertOverride } =
    useCommitDraft(agent.id, panelState);

  // This checkout's in-flight delegation, for display only — the lifecycle that
  // clears it runs at the app root. Its settled outcome arrives the same way:
  // `useDelegationSync` posts it to the store (it has no panel to write to), and
  // we merge it with this section's own action notices.
  const key = checkoutKey(agent.id, subdir);
  const delegation = useAppStore((s) => s.delegations[key]);
  const delegationNotice = useAppStore((s) => s.delegationNotices[key]);
  // Autopilot is how the agent behaves on this PR (switched per project in
  // settings, pausable per workspace from the header). The action bar's status
  // slot carries the one thing worth knowing about it: that the in-flight turn
  // was started automatically. What it did afterwards is the body's history.
  const autopilot = useAppStore((s) => s.autopilot[key]);
  const autoAttempt = autopilot?.cycle?.phase === "working" ? autopilot.cycle.attempt : null;

  // The changed file whose diff is open in place of the list; null shows the
  // list. Falls back to the list when the file leaves it (e.g. after a commit).
  const [viewing, setViewing] = useState<string | null>(null);
  useEffect(() => {
    setViewing((prev) => (prev && gitState?.files.some((f) => f.path === prev) ? prev : null));
  }, [gitState]);
  // A multi-repo agent's checkout paths are addressed as `<subdir>/<path>` (as
  // in the Code tab's tree), which is how `get_file_diff` picks the checkout.
  const repoDir = agent.repos.length > 1 ? repo?.subdir : undefined;

  const githubConnected = useAppStore((s) => s.github?.authenticated ?? false);
  const hasOrigin = gitState?.has_origin ?? true;

  const branch = gitState?.branch || repo?.branch || "(no branch yet)";
  const base = gitState?.parent_branch || repo?.parent_branch || "main";
  // The checkout is detached until its first push; a branch is only born from
  // an agent that names it. So a direct (agent-bypassed) action that needs a
  // branch — push, open PR — can't run yet: it routes through the agent
  // instead, which picks a conventional name and creates the branch.
  const hasBranch = Boolean(gitState?.branch || repo?.branch);

  const { runAction, addCommentToChat } = useGitActions({
    agentId: agent.id,
    subdir,
    base,
    hasBranch,
    customActive,
    msg,
    checks,
    prUrl: prState?.url,
    githubConnected,
    hasOrigin,
    runBusy,
    showNotice,
    openOverride,
    revertOverride,
    fetchPrState,
  });

  const { primary, items, effectiveKey, tone, mainDisabled, mainReason, onSelectAction } =
    useActionBarModel({
      agentId: agent.id,
      panelState,
      gitState,
      prState,
      checks,
      mergeState,
      prOpen,
      base,
      customActive,
      delegationActive: delegation != null,
      githubConnected,
    });

  // Pushed state: link the commit count out to GitHub — a single commit when
  // only one is ahead, otherwise the base..branch compare (commit list + full
  // diff). Gated on nothing being unpushed, so the tip is on origin and the
  // link can't 404. Needs the origin web base (github.com remotes only).
  const webBase = gitState?.remote_url ?? null;
  const aheadCount = gitState?.ahead ?? 0;
  const unpushed = gitState?.unpushed ?? 0;
  const pushedLink: string | null =
    webBase && unpushed === 0 && aheadCount > 0
      ? aheadCount === 1 && gitState?.head_sha
        ? `${webBase}/commit/${gitState.head_sha}`
        : `${webBase}/compare/${base}...${branch}`
      : null;

  // Show the changes list only when there are uncommitted files to display.
  // The commit composer yields while the agent holds a delegation.
  const showFiles = panelState === "changes" || panelState === "conflicts";
  const showCommit = panelState === "changes" && !delegation;
  // Keys Fletch refuses to run git over here. While any remain, the git state is
  // stale and every action would fail, so the header says so, the card replaces
  // the body and the footer steps aside.
  const blockedConfig = useAppStore((s) => s.gitBlocked[key]);
  const blocked = blockedConfig != null;

  // The PRs this checkout held before the current one — a workspace that kept
  // working after a merge has them. Each is a linked pill, so landed work stays
  // reachable once the header has moved on to the follow-up.
  const priorPrs = usePrHistory(agent.id, prState?.number ?? null, subdir);

  return (
    <div className="git-wrap">
      {/* ── color-coded status header: the at-a-glance state signal ── */}
      <StatusHeader
        state={panelState}
        branch={branch}
        base={base}
        git={gitState}
        pr={prState}
        mergeState={mergeState}
        checksFailed={checks?.failed ?? 0}
        blocked={blocked}
        controls={
          autopilotSwitch && <AutopilotSwitch agentId={agent.id} projectId={agent.project_id} />
        }
      />

      {/* Earlier PRs of this checkout, once it has any — merged work stays one
          click away after the panel moves on to the follow-up. */}
      {priorPrs.length > 0 && (
        <PrSetStrip
          heading="Earlier"
          entries={priorPrs.map((pr) => ({
            key: String(pr.number),
            // Backfilled rows can carry an empty title (the pre-history schema
            // stored no snapshot until a fetch succeeded) — fall back to the
            // number so the tooltip never reads as a bare " · merged".
            context: pr.title || `PR #${pr.number}`,
            pr,
            // Settled PRs have no live CI to show; the pill reads its state.
            checks: null,
          }))}
        />
      )}

      {/* ── scrollable body: the changes are the focus ── */}
      <div className={`git-body ${busy ? "busy" : ""}`}>
        {blockedConfig ? (
          <BlockedConfigCard
            keys={blockedConfig}
            busy={busy != null}
            onRemove={() => runAction("clear-config")}
          />
        ) : showFiles && viewing ? (
          <FileDiffView
            agentId={agent.id}
            files={gitState?.files ?? []}
            path={viewing}
            repoDir={repoDir}
            onSelect={setViewing}
            onBack={() => setViewing(null)}
          />
        ) : (
          <>
            {panelState === "pr-open" && prState && (
              <PRCard
                pr={prState}
                base={base}
                checks={checks}
                comments={comments}
                onAddToChat={addCommentToChat}
              />
            )}
            {panelState === "pr-closed" && prState && <ClosedPRCard pr={prState} />}
            {panelState === "conflicts" && gitState && <ConflictCard files={gitState.files} />}

            {showFiles && (
              <ChangesList
                files={gitState?.files ?? []}
                onOpen={setViewing}
                onRefresh={() => void fetchGitState(agent.id)}
              />
            )}

            {/* What the agent did on this PR by itself — with the PR/changes it
             *  describes. Renders nothing until it has actually done something. */}
            <AutopilotHistory agentId={agent.id} subdir={subdir} />

            <EmptyState state={panelState} base={base} />
          </>
        )}
      </div>

      {/* ── pinned footer: one row of status + action, with the commit
             message field unfolding above it only when the user opts in ── */}
      <div className="git-foot" hidden={blocked}>
        {showCommit && (
          <CommitComposer
            writing={override}
            msg={msg}
            setMsg={setMsg}
            textareaRef={commitRef}
            onRevert={revertOverride}
            // Cmd/Ctrl+Enter in the field is the main button's twin, so it
            // inherits its disabled state: the environment can't run the
            // selected action, the merge gate is shut, or a delegation holds
            // the checkout. `runAction` refuses the first of those by itself,
            // but the shortcut should be as dead as the button for all three.
            onSubmit={() => {
              if (!mainDisabled) runAction(effectiveKey);
            }}
          />
        )}

        <ActionBar
          statusKind={primary.statusKind}
          statusLabel={primary.statusLabel}
          // In the changes state the status is who writes the message — the
          // header and the button already say "uncommitted" and "commit".
          idle={
            showCommit ? (
              <CommitStatus
                writing={override}
                hasMsg={msg.trim().length > 0}
                onOpen={openOverride}
              />
            ) : undefined
          }
          busy={busy}
          delegationLabel={delegation ? delegationLabel(delegation.kind) : null}
          autoAttempt={autoAttempt}
          notice={notice ?? delegationNotice ?? null}
          panelState={panelState}
          pushedLink={pushedLink}
          aheadCount={aheadCount}
          items={items}
          selectedKey={effectiveKey}
          tone={tone}
          mainDisabled={mainDisabled}
          mainReason={mainReason}
          onSelect={onSelectAction}
          onRun={() => runAction(effectiveKey)}
        />
      </div>
    </div>
  );
}
