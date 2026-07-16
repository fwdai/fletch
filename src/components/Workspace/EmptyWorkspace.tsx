import { useState } from "react";
import { api } from "@/api";
import { Composer } from "@/components/Composer";
import { BranchPicker } from "@/components/Composer/BranchPicker";
import { ProjectPicker } from "@/components/Composer/ProjectPicker";
import { Icon, LandmarkGlyph } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import type { DraftAgent } from "@/store";
import { useAppStore } from "@/store";
import { useDefinitions } from "@/workflows/run/useDefinitions";
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
  const repos = useAppStore((s) => s.workspace?.repos ?? []);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);
  const runLocalCommand = useAppStore((s) => s.runLocalCommand);

  // Kickoff mode: a single agent, or a workflow. The toggle sits at the top of
  // the page and swaps the whole block — it is not part of the prompt box. The
  // Workflow option only appears once at least one workflow has been defined.
  const [mode, setMode] = useState<"agent" | "workflow">("agent");
  const { definitions } = useDefinitions();
  const hasWorkflows = definitions.length > 0;
  const workflowMode = mode === "workflow" && hasWorkflows;

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
        {hasWorkflows && (
          <div className="empty-modeswitch">
            <div className="set-seg">
              <button className={mode === "agent" ? "active" : ""} onClick={() => setMode("agent")}>
                <Icon name="bot" size={13} /> Agent
              </button>
              <button
                className={mode === "workflow" ? "active" : ""}
                onClick={() => setMode("workflow")}
              >
                <Icon name="combine" size={13} /> Workflow
              </button>
            </div>
          </div>
        )}

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
                A checkout and sandbox will be created at{" "}
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
              />
            ) : (
              <Composer
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
                repos={repos}
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
