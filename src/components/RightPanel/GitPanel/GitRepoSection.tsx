import { open } from "@tauri-apps/plugin-shell";
import { useEffect, useMemo, useState } from "react";
import type { AgentRecord, TrackedRepo } from "@/api";
import { delegationLabel } from "@/delegation";
import { useAppStore } from "@/store";
import { checkoutKey } from "@/store/git";
import { ActionBar } from "./ActionBar";
import { AutopilotHistory, AutopilotSwitch } from "./Autopilot";
import { CommitComposer, CommitStatus } from "./CommitComposer";
import { BlockedConfigCard } from "./cards";
import { FileDiffView } from "./FileDiffView";
import { useActionBarModel } from "./hooks/useActionBarModel";
import { useCommitDraft } from "./hooks/useCommitDraft";
import { useGitActions } from "./hooks/useGitActions";
import { useGitPanelData } from "./hooks/useGitPanelData";
import { usePrOrigin } from "./hooks/usePrOrigin";
import { useTransientFeedback } from "./hooks/useTransientFeedback";
import { Ladder } from "./Ladder";
import { Lanes } from "./Lanes";
import { buildLadder, buildPrLadder } from "./ladderModel";
import { buildLanes, defaultLane } from "./laneModel";
import type { SplitActionItem } from "./SplitAction";
import { WorktreesNote } from "./WorktreesNote";

/** The footer of a lane the panel can only look at — another PR of the
 *  checkout than the focused one. The host's PR actions and autopilot work on
 *  the focused PR, so acting on this one means focusing it first; that is the
 *  one place the panel moves the focus, and it says so. */
const LANE_ITEMS: SplitActionItem[] = [
  { key: "focus-pr", label: "Focus this PR", icon: "pr" },
  { key: "view-pr", label: "View on GitHub", icon: "github" },
];

/** One repo's worth of git panel: lanes (header) · ladder (body) · action bar
 *  (footer), scoped to a single checkout of the agent. For a single-repo agent
 *  this IS the whole panel. `subdir` is undefined for the agent's primary repo
 *  (index 0) — that section reads/writes the plain agent-keyed store entries
 *  that live events and bulk polls update — and the checkout's directory name
 *  for secondaries, which read/write under `checkoutKey(agentId, subdir)`.
 *
 *  The component is a thin orchestrator: live reads + polling live in
 *  `useGitPanelData`, the commit draft in `useCommitDraft`, busy/notice in
 *  `useTransientFeedback`, the dispatch table in `useGitActions`, the
 *  split-button model in `useActionBarModel`, and the two projections the body
 *  draws in `lanes.ts` / `ladder.ts`. The agent-handoff lifecycle is NOT here —
 *  it belongs to the host (`supervisor::delegation`), so a delegation completes
 *  whether or not this section, or any window, is on screen; the section only
 *  reads the store's mirror of it to render it. */
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
  // clears it runs on the host. Its settled outcome arrives the same way: the
  // host's `done` / `abandoned` event posts it to the store, and we merge it
  // with this section's own action notices.
  const key = checkoutKey(agent.id, subdir);
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

  const { items, effectiveKey, tone, mainDisabled, mainReason, onSelectAction } = useActionBarModel(
    {
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
    },
  );

  // ── Lanes: every branch-and-PR of the checkout, and the one in view ────────
  // The PR set holds every PR of the checkout (sub-agents each open their own;
  // a workspace that kept working after a merge holds the merged one too).
  const prSet = useAppStore((s) => s.prSets[key]);
  const lanes = useMemo(
    () => buildLanes(prSet, prState, checks, gitState),
    [prSet, prState, checks, gitState],
  );
  // Which lane the ladder describes: the user's pick while it exists, else the
  // default (the focused PR's, or the tree's). Local to this section — viewing
  // a lane never moves the host's focus.
  // The pick remembers which agent it was made for, so a switch to another
  // workspace starts from that one's default rather than a stale lane id.
  const [pick, setPick] = useState<{ agentId: string; laneId: string } | null>(null);
  const picked = pick?.agentId === agent.id ? pick.laneId : null;
  const viewed = (picked ? lanes.find((l) => l.id === picked) : undefined) ?? defaultLane(lanes);
  // The lane the footer's actions apply to: the focused PR's lane, or the
  // tree's own lane when no PR covers its branch. Any other lane can only be
  // looked at — and focused.
  const acting = viewed == null || viewed.pr == null || viewed.focused;
  // The PR in view with everything the store knows about it: the focused PR's
  // live state when it is the one in view, else the set entry's snapshot.
  const viewedPr = viewed?.pr ? (acting ? prState : viewed.pr.state) : null;
  const viewedChecks = viewed?.pr ? (acting ? checks : viewed.pr.checks) : null;

  const rows = useMemo(() => {
    if (!viewed) return [];
    if (!acting && viewed.pr) return buildPrLadder(viewed.pr, base);
    // The tree's own lane while another PR is focused describes the tree with
    // no proposal: whatever it commits lands on its branch, not on that PR.
    const withPr = viewed.pr != null;
    return buildLadder({
      git: gitState,
      pr: withPr ? prState : null,
      checks: withPr ? checks : null,
      mergeState: withPr ? mergeState : null,
      comments: withPr ? comments : null,
      base,
    });
  }, [viewed, acting, gitState, prState, checks, mergeState, comments, base]);

  // The focus action for a lane that isn't the focused PR's.
  const focusPr = useAppStore((s) => s.focusPr);
  const [laneKey, setLaneKey] = useState("focus-pr");
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
  const showCommit = acting && panelState === "changes" && !delegation;
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
        onView={(laneId) => setPick({ agentId: agent.id, laneId })}
        git={gitState}
        controls={
          autopilotSwitch && <AutopilotSwitch agentId={agent.id} projectId={agent.project_id} />
        }
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
                }}
              />
            )}

            {/* What the agent did on this PR by itself — under the ladder it
             *  describes. Renders nothing until it has actually done something. */}
            <AutopilotHistory agentId={agent.id} subdir={subdir} />
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
            items={items}
            selectedKey={effectiveKey}
            tone={tone}
            mainDisabled={mainDisabled}
            mainReason={mainReason}
            onSelect={onSelectAction}
            onRun={() => runAction(effectiveKey)}
          />
        ) : (
          <ActionBar
            idle="Merge, checks and autopilot follow the focused PR"
            busy={busy}
            delegationLabel={delegation ? delegationLabel(delegation.kind) : null}
            autoAttempt={autoAttempt}
            notice={notice ?? delegationNotice ?? null}
            items={LANE_ITEMS}
            selectedKey={laneKey}
            tone="ghost"
            mainDisabled={false}
            onSelect={setLaneKey}
            onRun={() => runLaneAction(laneKey)}
          />
        )}
      </div>
    </div>
  );
}
