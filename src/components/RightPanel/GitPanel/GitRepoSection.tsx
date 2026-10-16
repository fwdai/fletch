import { open } from "@tauri-apps/plugin-shell";
import { useEffect, useMemo, useState } from "react";
import type { AgentRecord, TrackedRepo } from "@/api";
import { isCommitAction } from "@/components/RightPanel/primaryActions";
import { delegationLabel } from "@/delegation";
import { hasWorkSinceMerge } from "@/readiness";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
import { ActionBar } from "./ActionBar";
import { AutopilotHistory, AutopilotSwitch } from "./Autopilot";
import { CommitComposer, CommitStatus } from "./CommitComposer";
import { BlockedConfigCard } from "./cards";
import { FileDiffView } from "./FileDiffView";
import type { FooterAction, FooterGroup } from "./footerModel";
import { useCommitDraft } from "./hooks/useCommitDraft";
import { useFooterModel } from "./hooks/useFooterModel";
import { useGitActions } from "./hooks/useGitActions";
import { useGitPanelData } from "./hooks/useGitPanelData";
import { usePrOrigin } from "./hooks/usePrOrigin";
import { useTransientFeedback } from "./hooks/useTransientFeedback";
import { Ladder } from "./Ladder";
import { Lanes } from "./Lanes";
import { buildLadder, buildPrLadder } from "./ladderModel";
import type { Lane } from "./laneModel";
import { branchMismatch } from "./mainAction";
import { WorktreesNote } from "./WorktreesNote";

/** The footer of a lane the panel can only look at — another PR of the
 *  checkout than the focused one. The host's PR actions and autopilot work on
 *  the focused PR, so acting on this one means focusing it first; that is the
 *  one place the panel moves the focus, and it says so. */
const LANE_PRIMARY: FooterAction = {
  key: "focus-pr",
  label: "Focus this PR",
  icon: "pr",
  tone: "ghost",
};
const LANE_GROUPS: FooterGroup[] = [
  { items: [{ key: "view-pr", label: "View on GitHub", icon: "github" }] },
];

/** The git panel: lanes (header) · ladder (body) · action bar (footer). The
 *  lanes span the workspace (`GitPanel` builds them); the ladder and the footer
 *  describe and act on the checkout the viewed lane belongs to — `repo` and
 *  `subdir`, undefined for the agent's primary repo (index 0), which reads and
 *  writes the plain agent-keyed store entries that live events and bulk polls
 *  update; secondaries read/write under `checkoutKey(agentId, subdir)`.
 *
 *  The component is a thin orchestrator: live reads + polling live in
 *  `useGitPanelData`, the commit draft in `useCommitDraft`, busy/notice in
 *  `useTransientFeedback`, the dispatch table in `useGitActions`, the footer
 *  in `useFooterModel`, and the projections the body draws in `laneModel.ts` /
 *  `ladderModel.ts`. The agent-handoff lifecycle is NOT here — it belongs to
 *  the host (`supervisor::delegation`), so a delegation completes whether or
 *  not this section, or any window, is on screen; the section only reads the
 *  store's mirror of it to render it. */
export function GitRepoSection({
  agent,
  repo,
  subdir,
  lanes,
  viewed,
  onView,
}: {
  agent: AgentRecord;
  repo: TrackedRepo | undefined;
  subdir?: string;
  /** Every lane of the workspace, and the one the ladder describes. */
  lanes: Lane[];
  viewed: Lane | null;
  onView: (laneId: string) => void;
}) {
  const { gitState, prState, checks, comments, mergeState, fetchGitState, fetchPrState } =
    useGitPanelData(agent.id, repo, subdir);

  const key = checkoutKey(agent.id, subdir);
  const { busy, runBusy, notice, showNotice } = useTransientFeedback(agent.id);
  // Something to commit: uncommitted files, none of them conflicted (a
  // conflict is the agent's to reconcile first, not a message to write).
  const committable =
    (gitState?.files.length ?? 0) > 0 && !gitState?.files.some((f) => f.kind === "conflicted");
  // A half-written message is this checkout's: a multi-repo workspace drops
  // it when the lane in view moves to another repo, never carrying it across.
  const { override, msg, setMsg, commitRef, customActive, openOverride, revertOverride } =
    useCommitDraft(key, committable);

  // This checkout's in-flight delegation, for display only — the lifecycle that
  // clears it runs on the host. Its settled outcome arrives the same way: the
  // host's `done` / `abandoned` event posts it to the store, and we merge it
  // with this section's own action notices.
  const delegation = useAppStore((s) => s.delegations[key]);
  const delegationNotice = useAppStore((s) => s.delegationNotices[key]);
  // Autopilot is how the agent behaves on the focused PR (switched per project
  // in settings, pausable per workspace from the lanes). The action bar's
  // status slot carries the one thing worth knowing about it: that the
  // in-flight turn was started automatically. What it did afterwards is the
  // body's history.
  const autopilot = useAppStore((s) => s.autopilot[key]);
  const autoAttempt = autopilot?.cycle?.phase === "working" ? autopilot.cycle.attempt : null;

  const githubConnected = useAppStore((s) => s.github?.authenticated ?? false);
  const hasOrigin = gitState?.has_origin ?? true;

  const branch = gitState?.branch || repo?.branch || "";
  const base = gitState?.parent_branch || repo?.parent_branch || "main";
  // The checkout is detached until its first push; a branch is only born from
  // an agent that names it. So a direct (agent-bypassed) action that needs a
  // branch — push, open PR — can't run yet: it routes through the agent
  // instead, which picks a conventional name and creates the branch.
  const hasBranch = Boolean(branch);

  // ── The lane in view ───────────────────────────────────────────────────────
  // The lane the footer's actions apply to: the focused PR's lane, or the
  // tree's own lane when no PR covers its branch. Any other lane can only be
  // looked at — and focused.
  const acting = viewed == null || viewed.pr == null || viewed.focused;
  // What the ladder AND the footer describe, so they can never disagree: the
  // focused PR's live state on its own lane; nothing on the tree's bare lane
  // while another PR is focused (a commit from there lands on its branch, a
  // push from there wants its own proposal — not that PR's merge).
  const laneHasPr = viewed?.pr != null;
  const lanePr = laneHasPr ? prState : null;
  const laneChecks = laneHasPr ? checks : null;
  const laneMergeState = laneHasPr ? mergeState : null;
  const laneComments = laneHasPr ? comments : null;
  // The PR in view with everything the store knows about it: the focused PR's
  // live state when it is the one in view, else the set entry's snapshot.
  const viewedPr = viewed?.pr ? (acting ? prState : viewed.pr.state) : null;
  const viewedChecks = viewed?.pr ? (acting ? checks : viewed.pr.checks) : null;

  const { runAction, addCommentToChat } = useGitActions({
    agentId: agent.id,
    subdir,
    base,
    hasBranch,
    customActive,
    msg,
    checks: laneChecks,
    comments: laneComments,
    prUrl: lanePr?.url,
    githubConnected,
    hasOrigin,
    runBusy,
    showNotice,
    openOverride,
    revertOverride,
    fetchPrState,
  });

  // The footer: the ladder's next step as the primary, the rest as a menu.
  const { primary, groups, tone, mainDisabled, mainReason } = useFooterModel({
    gitState,
    prState: lanePr,
    checks: laneChecks,
    mergeState: laneMergeState,
    comments: laneComments,
    base,
    customActive,
    delegationActive: delegation != null,
    githubConnected,
  });
  const setCommitMode = useAppStore((s) => s.setGitCommitAction);
  // A menu row: a commit mode becomes the sticky default (and so, while there
  // is something to commit, the primary); anything else runs.
  const onPick = (k: string) => {
    if (isCommitAction(k)) setCommitMode(k);
    else runAction(k);
  };

  const rows = useMemo(() => {
    if (!viewed) return [];
    if (!acting && viewed.pr) return buildPrLadder(viewed.pr, base);
    return buildLadder({
      git: gitState,
      pr: lanePr,
      checks: laneChecks,
      mergeState: laneMergeState,
      comments: laneComments,
      base,
    });
  }, [viewed, acting, gitState, lanePr, laneChecks, laneMergeState, laneComments, base]);

  // The focus action for a lane that isn't the focused PR's.
  const focusPr = useAppStore((s) => s.focusPr);
  const runLaneAction = (k: string) => {
    const pr = viewed?.pr?.state;
    if (!pr) return;
    if (k === "focus-pr") void focusPr(agent.id, pr.number, subdir);
    else void open(pr.url);
  };

  // ── Ladder bodies ──────────────────────────────────────────────────────────
  // Pushed commits link out to GitHub — a single commit when only one is ahead,
  // otherwise the base..branch compare (commit list + full diff). Gated on
  // nothing being unpushed, so the tip is on origin and the link can't 404.
  // Needs the origin web base (github.com remotes only).
  const webBase = gitState?.remote_url ?? null;
  const aheadCount = gitState?.ahead ?? 0;
  const unpushed = gitState?.unpushed ?? 0;
  const commitsLink: string | null =
    webBase && unpushed === 0 && aheadCount > 0
      ? aheadCount === 1 && gitState?.head_sha
        ? `${webBase}/commit/${gitState.head_sha}`
        : `${webBase}/compare/${base}...${branch}`
      : null;
  // The PR's head branch, said on its row when the tree is on another one —
  // the same rule that gates the actions bound to that branch. Not before the
  // git state loads, so the line doesn't flash on every open.
  const prBranch =
    gitState && viewedPr?.branch && viewedPr.branch !== gitState.branch ? viewedPr.branch : null;
  // Which sub-agent opened the PR in view, when one did — its row says so.
  const prOrigin = usePrOrigin(agent.id, viewedPr?.number);
  // A merged PR's row ends its branch. Archiving removes the clone and deleting
  // the branch its ref, so both are dead while work since the merge is still
  // only here (uncommitted, or commits origin lacks) — and from a tree on
  // another branch, which they say (the footer's branch-bound rule).
  const ending: Parameters<typeof Ladder>[0]["bodies"]["ending"] = acting
    ? {
        onArchive: () => runAction("archive"),
        onDeleteBranch: () => runAction("delete-branch"),
        reason:
          branchMismatch({
            effectiveKey: "archive",
            prBranch: lanePr?.branch ?? null,
            checkoutBranch: gitState?.branch || null,
          }) ??
          (gitState && hasWorkSinceMerge(gitState) ? "Work since the merge is only here" : null),
      }
    : null;

  // The changed file whose diff is open in place of the ladder; null shows the
  // ladder. Falls back when the file leaves the list (e.g. after a commit).
  const [viewing, setViewing] = useState<string | null>(null);
  useEffect(() => {
    setViewing((prev) => (prev && gitState?.files.some((f) => f.path === prev) ? prev : null));
  }, [gitState]);
  // A multi-repo agent's checkout paths are addressed as `<subdir>/<path>` (as
  // in the Code tab's tree), which is how `get_file_diff` picks the checkout.
  const repoDir = agent.repos.length > 1 ? repo?.subdir : undefined;

  // The commit composer yields while the agent holds a delegation, and only
  // belongs to the lane whose tree it commits.
  const showCommit = acting && committable && !delegation;
  // Keys Fletch refuses to run git over here. While any remain, the git state is
  // stale and every action would fail, so the card replaces the body and the
  // footer steps aside.
  const blockedConfig = useAppStore((s) => s.gitBlocked[key]);
  const blocked = blockedConfig != null;

  return (
    <div className="git-wrap">
      {/* ── lanes: the checkout's branches and PRs, one in view ── */}
      <Lanes
        lanes={lanes}
        viewedId={viewed?.id ?? null}
        onView={onView}
        controls={<AutopilotSwitch agentId={agent.id} projectId={agent.project_id} />}
      />

      <WorktreesNote worktrees={gitState?.worktrees} />

      {/* ── scrollable body: the lane's way to landing ── */}
      <div className={`git-body ${busy ? "busy" : ""}`}>
        {blockedConfig ? (
          <BlockedConfigCard
            keys={blockedConfig}
            busy={busy != null}
            onRemove={() => runAction("clear-config")}
          />
        ) : viewing ? (
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
            {viewed && (
              <Ladder
                key={viewed.id}
                rows={rows}
                bodies={{
                  files: acting ? (gitState?.files ?? []) : [],
                  onOpenFile: setViewing,
                  onRefresh: () => void fetchGitState(agent.id),
                  commitsLink: acting ? commitsLink : null,
                  commitsAhead: aheadCount,
                  pr: viewedPr,
                  prBranch,
                  origin: prOrigin,
                  checks: viewedChecks,
                  comments: acting ? comments : null,
                  onAddToChat: addCommentToChat,
                  ending,
                }}
              />
            )}

            {/* What the agent did on this PR by itself — under the ladder it
             *  describes. Renders nothing until it has actually done something. */}
            {acting && <AutopilotHistory agentId={agent.id} subdir={subdir} />}
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
              if (!mainDisabled) runAction(primary.key);
            }}
          />
        )}

        {acting ? (
          <ActionBar
            // In the changes state the resting note is who writes the message —
            // the ladder already says "3 files" and the button says "commit".
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
            primary={primary}
            groups={groups}
            tone={tone}
            mainDisabled={mainDisabled}
            mainReason={mainReason}
            onPick={onPick}
            onRun={() => runAction(primary.key)}
          />
        ) : (
          <ActionBar
            idle="Merge, checks and autopilot follow the focused PR"
            busy={busy}
            delegationLabel={delegation ? delegationLabel(delegation.kind) : null}
            autoAttempt={autoAttempt}
            notice={notice ?? delegationNotice ?? null}
            primary={LANE_PRIMARY}
            groups={LANE_GROUPS}
            tone="ghost"
            mainDisabled={false}
            onPick={runLaneAction}
            onRun={() => runLaneAction("focus-pr")}
          />
        )}
      </div>
    </div>
  );
}
