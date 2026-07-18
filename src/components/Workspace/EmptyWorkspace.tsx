import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "@/api";
import { Composer } from "@/components/Composer";
import { BranchPicker } from "@/components/Composer/BranchPicker";
import { type ProjectOption, ProjectPicker } from "@/components/Composer/ProjectPicker";
import { Icon, LandmarkGlyph } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import {
  type ComposerMode,
  loadPipelinePrefs,
  rememberComposerMode,
} from "@/workflows/run/projectPipeline";
import { WorkflowComposer, WorkflowHeading } from "@/workflows/run/WorkflowComposer";

/** Empty-state pane shown when the user has started a draft agent.
 *  First message in the composer spawns the real agent and dispatches
 *  the message in one go. */
export function EmptyWorkspace({ draft }: { draft: DraftAgent }) {
  const rerollDraftName = useAppStore((s) => s.rerollDraftName);
  const spawnFromDraft = useAppStore((s) => s.spawnFromDraft);
  const removeDraft = useAppStore((s) => s.removeDraft);
  const updateDraft = useAppStore((s) => s.updateDraft);
  const setNewDraftSelection = useAppStore((s) => s.setNewDraftSelection);
  const setLastRepoPath = useAppStore((s) => s.setLastRepoPath);
  const projectRefs = useAppStore((s) => s.workspace?.projects ?? []);
  // One picker entry per project: a multi-repo project shows once, valued at
  // its primary (first) repo, which is where the agent spawns.
  const projectOptions = useMemo(() => {
    const seen = new Map<string, ProjectOption>();
    for (const ref of projectRefs) {
      const key = ref.project_id || ref.path;
      if (!seen.has(key)) seen.set(key, { path: ref.path, label: ref.name });
    }
    return [...seen.values()];
  }, [projectRefs]);
  // How many repos the drafted project spans — a multi-repo project gets a
  // checkout of each at spawn, and the sub-copy should say so.
  const repoCount = useMemo(() => {
    const projectId = projectRefs.find((r) => r.path === draft.repoPath)?.project_id;
    if (!projectId) return 1;
    return projectRefs.filter((r) => r.project_id === projectId).length;
  }, [projectRefs, draft.repoPath]);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const runLocalCommand = useAppStore((s) => s.runLocalCommand);

  // The project this draft targets — keys the remembered composer mode + default
  // workflow (per-project `project_settings`).
  const projectId = useMemo(
    () => projectRefs.find((r) => r.path === draft.repoPath)?.project_id ?? "",
    [projectRefs, draft.repoPath],
  );

  // Kickoff mode: a quick single agent, or a multi-step pipeline. The toggle
  // sits at the top of the page and swaps the whole block — it never gates the
  // quick path. It defaults to the project's remembered choice (a suggestion),
  // and remembers a change for next time. The remembered mode loads async — a
  // pick made before it resolves wins (the load must never flip the toggle
  // back under the user's cursor); a project switch re-arms the default.
  const [mode, setMode] = useState<ComposerMode>("agent");
  const [defaultWorkflowId, setDefaultWorkflowId] = useState<string | null>(null);
  const modePicked = useRef(false);
  useEffect(() => {
    modePicked.current = false;
    let cancelled = false;
    void loadPipelinePrefs(projectId).then((prefs) => {
      if (cancelled) return;
      if (!modePicked.current) setMode(prefs.mode);
      setDefaultWorkflowId(prefs.defaultWorkflowId);
    });
    return () => {
      cancelled = true;
    };
  }, [projectId]);
  const pickMode = (next: ComposerMode) => {
    modePicked.current = true;
    setMode(next);
    rememberComposerMode(projectId, next);
  };
  const workflowMode = mode === "workflow";

  return (
    <div className="pane center">
      <div className="center-h flex-center">
        <IconButton
          tip={leftCollapsed ? "Show sidebar (⌘B)" : "Hide sidebar (⌘B)"}
          onClick={toggleLeft}
        >
          <Icon name="sidebarL" />
        </IconButton>
        <div className="task" style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
          <span
            style={{
              fontSize: "var(--fs-xs)",
              color: "var(--fg-3)",
              fontFamily: "var(--font-mono)",
              textTransform: "uppercase",
              letterSpacing: "0.08em",
            }}
          >
            Drafting
          </span>
          <span className="serif" style={{ fontSize: "var(--fs-lg)", color: "var(--accent)" }}>
            {draft.name}
          </span>
        </div>
        <IconButton tip="Discard draft" onClick={() => removeDraft(draft.id)}>
          <Icon name="close" />
        </IconButton>
      </div>

      <div className="empty-wrap flex-center fade-in">
        <div className="empty-modeswitch">
          <div className="set-seg">
            <button className={mode === "agent" ? "active" : ""} onClick={() => pickMode("agent")}>
              <Icon name="bot" size={13} /> Quick agent
            </button>
            <button
              className={mode === "workflow" ? "active" : ""}
              onClick={() => pickMode("workflow")}
            >
              <Icon name="combine" size={13} /> Pipeline
            </button>
          </div>
        </div>

        {/* Everything but the switcher — identity, the mode-variable heading +
            composer, and the pickers — centered as one unit and keyed by mode so
            switching cross-fades it in place. The switcher is pinned separately
            (it never moves regardless of this block's height). */}
        <div className="empty-body fade-in" key={mode}>
          <div className="empty-id flex-center">
            <div className="empty-mark flex-center text-xs">
              <span className="d" />
              <span>NEW WORKSPACE</span>
            </div>
            <div
              className="empty-name text-5xl"
              onClick={() => rerollDraftName(draft.id)}
              title="Reroll name"
            >
              <LandmarkGlyph
                name={draft.name}
                size={36}
                strokeWidth={1.1}
                style={{ display: "inline-block", verticalAlign: "-6px", marginRight: 6 }}
              />
              {draft.name}
            </div>
          </div>

          {workflowMode ? (
            <WorkflowHeading />
          ) : (
            <>
              <h1 className="empty-title text-5xl">What should be the first task?</h1>
              <p className="empty-sub text-base">
                {repoCount > 1
                  ? `Checkouts of all ${repoCount} repositories and a sandbox will be created at `
                  : "A checkout and sandbox will be created at "}
                <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-1)" }}>
                  ~/.fletch/workspaces/{draft.name}
                </span>
              </p>
            </>
          )}

          <div className="empty-composer">
            {workflowMode ? (
              <WorkflowComposer
                repoPath={draft.repoPath}
                baseBranch={draft.base}
                name={draft.name}
                projectId={projectId}
                defaultWorkflowId={defaultWorkflowId}
                issueRef={draft.issueRef}
              />
            ) : (
              <Composer
                // Remount when the target repo changes so the @/# mention
                // sources drop the previous repo's cached files/PRs (they only
                // refetch on menu-open, not on a repoPath change under a live
                // composer) — otherwise a wrong-repo file/PR could be inserted.
                key={draft.repoPath}
                autoFocus
                draftKey={draft.id}
                defaultProvider={draft.provider}
                projectDir={draft.repoPath}
                onLocalCommand={(action) => runLocalCommand(action)}
                mentionSource={() => api.listRepoTree(draft.repoPath)}
                listDir={api.listDir}
                listPrs={() => api.listRepoPrs(draft.repoPath)}
                defaultModel={draft.model}
                defaultCustomAgentId={draft.customAgentId}
                onChangeSelection={(provider, model, customAgentId) => {
                  updateDraft(draft.id, { provider, model, customAgentId });
                  setNewDraftSelection(provider, model, customAgentId);
                }}
                placeholder="Describe the task for the agent. ↵ to spawn."
                onSend={({ text, provider, model, attachments, thinking, customAgentId }) =>
                  spawnFromDraft(
                    draft.id,
                    text,
                    provider,
                    model,
                    attachments,
                    thinking,
                    customAgentId,
                  )
                }
              />
            )}
            <div className="empty-meta flex-center">
              <ProjectPicker
                value={draft.repoPath}
                projects={projectOptions}
                onChange={(repoPath) => {
                  // Switching projects: the previously chosen base branch may not
                  // exist in the new repo, so reset to main.
                  updateDraft(draft.id, { repoPath, base: "main" });
                  setLastRepoPath(repoPath);
                }}
              />
              <BranchPicker
                repoPath={draft.repoPath}
                value={draft.base}
                onChange={(branch) => updateDraft(draft.id, { base: branch })}
              />
              <span className="pill is-action" onClick={() => rerollDraftName(draft.id)}>
                <Icon name="refresh" />
                <span>reroll name</span>
              </span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
