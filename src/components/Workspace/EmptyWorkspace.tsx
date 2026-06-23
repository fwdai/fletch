import type { DraftAgent } from "../../store";
import { useAppStore } from "../../store";
import { Icon, LandmarkGlyph } from "../Icon";
import { Composer } from "../Composer";
import { IconButton } from "../ui/IconButton";
import { basename } from "../../util/format";

/** Empty-state pane shown when the user has started a draft agent.
 *  First message in the composer spawns the real agent and dispatches
 *  the message in one go. */
export function EmptyWorkspace({ draft }: { draft: DraftAgent }) {
  const rerollDraftName = useAppStore((s) => s.rerollDraftName);
  const spawnFromDraft = useAppStore((s) => s.spawnFromDraft);
  const removeDraft = useAppStore((s) => s.removeDraft);
  const updateDraft = useAppStore((s) => s.updateDraft);
  const toggleLeft = useAppStore((s) => s.toggleLeft);
  const leftCollapsed = useAppStore((s) => s.leftCollapsed);

  const projectName = basename(draft.repoPath);

  return (
    <div className="pane center">
      <div className="center-h">
        <IconButton
          tip={leftCollapsed ? "Show sidebar (⌘B)" : "Hide sidebar (⌘B)"}
          onClick={toggleLeft}
        >
          <Icon name="sidebarL" />
        </IconButton>
        <div className="task" style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
          <span
            style={{
              fontSize: 11, color: "var(--fg-3)", fontFamily: "var(--font-mono)",
              textTransform: "uppercase", letterSpacing: "0.08em",
            }}
          >
            Drafting
          </span>
          <span className="serif" style={{ fontSize: 16, color: "var(--accent)" }}>
            {draft.name}
          </span>
        </div>
        <IconButton tip="Discard draft" onClick={() => removeDraft(draft.id)}>
          <Icon name="close" />
        </IconButton>
      </div>

      <div className="empty-wrap fade-in">
        <div className="empty-id">
          <div className="empty-mark">
            <span className="d" />
            <span>NEW WORKSPACE</span>
          </div>
          <div
            className="empty-name"
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

        <h1 className="empty-title">What should be the first task?</h1>
        <p className="empty-sub">
          A worktree and sandbox will be created at{" "}
          <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-1)" }}>
            ~/.quorum/worktrees/{draft.name}
          </span>
        </p>

        <div className="empty-composer">
          <Composer
            autoFocus
            draftKey={draft.id}
            defaultProvider={draft.provider}
            defaultModel={draft.model}
            baseBranch={draft.base}
            repoPath={draft.repoPath}
            onChangeBase={(branch) => updateDraft(draft.id, { base: branch })}
            onChangeSelection={(provider, model) =>
              updateDraft(draft.id, { provider, model })
            }
            placeholder="Describe the task for the agent. ↵ to spawn."
            onSend={({ text, provider, model, attachments, thinking }) =>
              spawnFromDraft(draft.id, text, provider, model, attachments, thinking)
            }
          />
          <div className="empty-meta">
            <span className="pill">
              <Icon name="folder" />
              <span className="v">{projectName}</span>
            </span>
            <span className="pill">
              <Icon name="branch" />
              <span style={{ color: "var(--fg-2)" }}>from</span>
              <span className="v">{draft.base}</span>
            </span>
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
