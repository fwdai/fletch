import { useEffect, useState } from "react";
import type { AgentRecord } from "@/api";
import { delegationLabel } from "@/components/RightPanel/delegation";
import { useAppStore } from "@/store";
import { ActionBar } from "./ActionBar";
import { ChangesList } from "./ChangesList";
import { CommitComposer } from "./CommitComposer";
import { ClosedPRCard, ConflictCard, PRCard } from "./cards";
import { EmptyState } from "./EmptyState";
import { useActionBarModel } from "./hooks/useActionBarModel";
import { useCommitDraft } from "./hooks/useCommitDraft";
import { useDelegationLifecycle } from "./hooks/useDelegationLifecycle";
import { useGitActions } from "./hooks/useGitActions";
import { useGitPanelData } from "./hooks/useGitPanelData";
import { useTransientFeedback } from "./hooks/useTransientFeedback";
import { StatusHeader } from "./StatusHeader";

/** State-aware git panel driven by live git state from the Tauri backend.
 *  Layout: a color-coded status header (the at-a-glance state signal), a
 *  scrollable body (the changes / PR card — the focus), and a pinned footer
 *  holding the commit message plus a responsive action bar (status left,
 *  split-button right; stacks full-width on a narrow panel via a container
 *  query). The panel is feature-flagged in settings.
 *
 *  The component is a thin orchestrator: live reads + polling live in
 *  `useGitPanelData`, the commit draft in `useCommitDraft`, busy/notice in
 *  `useTransientFeedback`, the agent-handoff lifecycle in
 *  `useDelegationLifecycle`, the dispatch table in `useGitActions`, and the
 *  split-button model in `useActionBarModel`. */
export function GitPanel({ agent }: { agent: AgentRecord }) {
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
    fetchPrChecks,
  } = useGitPanelData(agent.id);

  const { busy, runBusy, notice, showNotice } = useTransientFeedback(agent.id);
  const { override, msg, setMsg, commitRef, customActive, openOverride, revertOverride } =
    useCommitDraft(agent.id, panelState);

  const delegation = useDelegationLifecycle({
    agentId: agent.id,
    agentStatus: agent.status,
    gitState,
    prState,
    checks,
    showNotice,
    fetchPrChecks,
  });

  // Selected file in the changes list — kept valid across polls (fall back to
  // the first file when the selection disappears).
  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => {
    setSelected((prev) => {
      const paths = gitState?.files.map((f) => f.path) ?? [];
      if (prev && paths.includes(prev)) return prev;
      return paths[0] ?? null;
    });
  }, [gitState]);

  const githubConnected = useAppStore((s) => s.github?.authenticated ?? false);
  const hasOrigin = gitState?.has_origin ?? true;

  const branch = gitState?.branch || agent.repos[0]?.branch || "(no branch yet)";
  const base = gitState?.parent_branch || agent.repos[0]?.parent_branch || "main";
  // The checkout is detached until its first push; a branch is only born from
  // an agent that names it. So a direct (agent-bypassed) action that needs a
  // branch — push, open PR — can't run yet: it routes through the agent
  // instead, which picks a conventional name and creates the branch.
  const hasBranch = Boolean(gitState?.branch || agent.repos[0]?.branch);

  const { runAction, addCommentToChat } = useGitActions({
    agentId: agent.id,
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

  const { primary, items, effectiveKey, tone, mainDisabled, onSelectAction } = useActionBarModel({
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
      />

      {/* ── scrollable body: the changes are the focus ── */}
      <div className={`git-body ${busy ? "busy" : ""}`}>
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
            selected={selected}
            onSelect={setSelected}
            onRefresh={() => void fetchGitState(agent.id)}
          />
        )}

        <EmptyState state={panelState} base={base} />
      </div>

      {/* ── pinned footer: commit message + status + action ── */}
      <div className="git-foot">
        {showCommit && (
          <CommitComposer
            writing={override}
            msg={msg}
            setMsg={setMsg}
            textareaRef={commitRef}
            onOpen={openOverride}
            onRevert={revertOverride}
            onSubmit={() => runAction(effectiveKey)}
          />
        )}

        <ActionBar
          statusKind={primary.statusKind}
          statusLabel={primary.statusLabel}
          statusExtra={primary.statusExtra}
          busy={busy}
          delegationLabel={delegation ? delegationLabel(delegation.kind) : null}
          notice={notice}
          panelState={panelState}
          pushedLink={pushedLink}
          aheadCount={aheadCount}
          prUrl={prState?.url}
          items={items}
          selectedKey={effectiveKey}
          tone={tone}
          mainDisabled={mainDisabled}
          onSelect={onSelectAction}
          onRun={() => runAction(effectiveKey)}
        />
      </div>
    </div>
  );
}
