import type { DraftAgent } from "../../store";
import { useAppStore } from "../../store";
import { Composer } from "../Composer";
import { BranchPicker } from "../Composer/BranchPicker";
import { ProjectPicker } from "../Composer/ProjectPicker";
import { Icon, LandmarkGlyph } from "../Icon";
import { IconButton } from "../ui/IconButton";

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

        <h1 className="empty-title text-5xl">What should be the first task?</h1>
        <p className="empty-sub text-base">
          A worktree and sandbox will be created at{" "}
          <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-1)" }}>
            ~/.fletch/worktrees/{draft.name}
          </span>
        </p>

        <div className="empty-composer">
          <Composer
            autoFocus
            draftKey={draft.id}
            defaultProvider={draft.provider}
            defaultModel={draft.model}
            defaultCustomAgentId={draft.customAgentId}
            onChangeSelection={(provider, model, customAgentId) => {
              updateDraft(draft.id, { provider, model, customAgentId });
              setNewDraftSelection(provider, model, customAgentId);
            }}
            placeholder="Describe the task for the agent. ↵ to spawn."
            onSend={({ text, provider, model, attachments, thinking, customAgentId }) =>
              spawnFromDraft(draft.id, text, provider, model, attachments, thinking, customAgentId)
            }
          />
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
  );
}
