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
  const showLandmarks = useAppStore((s) => s.showLandmarks);
  const rerollDraftName = useAppStore((s) => s.rerollDraftName);
  const spawnFromDraft = useAppStore((s) => s.spawnFromDraft);
  const removeDraft = useAppStore((s) => s.removeDraft);
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
        <div className="empty-mark">
          <span className="d" />
          <span>NEW WORKSPACE</span>
          <span style={{ color: "var(--fg-3)" }}>· in</span>
          <span style={{ color: "var(--fg-1)", letterSpacing: 0, textTransform: "none" }}>
            {projectName}
          </span>
        </div>

        <h1 className="empty-title">
          What should{" "}
          <span
            className="name"
            onClick={() => rerollDraftName(draft.id)}
            title="Reroll name"
          >
            {showLandmarks && (
              <LandmarkGlyph
                name={draft.name}
                size={36}
                strokeWidth={1.1}
                style={{ display: "inline-block", verticalAlign: "-6px", marginRight: 6 }}
              />
            )}
            {draft.name}
          </span>
          <span> tackle?</span>
        </h1>
        <p className="empty-sub">
          A worktree and branch will be created under{" "}
          <span style={{ fontFamily: "var(--font-mono)", color: "var(--fg-1)" }}>
            ~/.quorum/worktrees/{draft.name}
          </span>
          . Your first message starts the agent.
        </p>

        <div className="empty-composer">
          <Composer
            autoFocus
            defaultProvider={draft.provider}
            baseBranch={draft.base}
            placeholder="Describe the task. ⌘↵ to spawn."
            onSend={({ text, provider }) =>
              spawnFromDraft(draft.id, text, provider)
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
            <span className="pill" onClick={() => rerollDraftName(draft.id)}>
              <Icon name="refresh" />
              <span>reroll name</span>
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
