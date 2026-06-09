import { ask } from "@tauri-apps/plugin-dialog";
import type { AgentRecord } from "../../api";
import type { DraftAgent } from "../../store";
import { useAppStore } from "../../store";
import { Icon } from "../Icon";
import { AgentRow } from "./AgentRow";

interface Props {
  /** Display name (basename of repo path). */
  label: string;
  /** Full repo path — used as the group's stable id. */
  repoPath: string;
  agents: AgentRecord[];
  drafts: DraftAgent[];
  /** Whether the user has expanded this group. */
  open: boolean;
  /** Show the remove (×) button — only true when this is a pinned-but-empty group. */
  removable: boolean;
  onToggle: () => void;
}

export function ProjectGroup({
  label, repoPath, agents, drafts, open, removable, onToggle,
}: Props) {
  const selectedAgentId = useAppStore((s) => s.selectedAgentId);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const showLandmarks = useAppStore((s) => s.showLandmarks);
  const selectAgent = useAppStore((s) => s.selectAgent);
  const selectDraft = useAppStore((s) => s.selectDraft);
  const createDraft = useAppStore((s) => s.createDraft);
  const removeWorkspaceRepo = useAppStore((s) => s.removeWorkspaceRepo);

  const count = agents.length + drafts.length;

  async function onRemove(e: React.MouseEvent) {
    e.stopPropagation();
    const ok = await ask(`Remove "${label}" from the sidebar?`, {
      title: "Remove repo",
      kind: "info",
    });
    if (ok) await removeWorkspaceRepo(repoPath);
  }

  function onAddAgent(e: React.MouseEvent) {
    e.stopPropagation();
    createDraft(repoPath);
  }

  return (
    <div className="proj">
      <div
        className={`proj-h ${open ? "open" : ""}`}
        onClick={onToggle}
        title={repoPath}
      >
        <Icon name="chevR" size={10} className="chev" />
        <span className="pname">{label}</span>
        <span className="pcount">{count}</span>
        <button
          className="padd tip"
          data-tip="New agent"
          data-tip-down=""
          onClick={onAddAgent}
        >
          <Icon name="plus" size={11} />
        </button>
        {removable && (
          <button
            className="padd tip"
            data-tip="Remove repo"
            data-tip-down=""
            onClick={onRemove}
            aria-label="Remove repo"
          >
            <Icon name="close" />
          </button>
        )}
      </div>

      <div className={`agents ${open ? "" : "closed"}`}>
        <button className="agent-new" onClick={onAddAgent}>
          <Icon name="plus" size={11} />
          <span>New agent</span>
        </button>
        {drafts.map((d) => (
          <AgentRow
            key={d.id}
            kind="draft"
            draft={d}
            active={d.id === activeDraftId}
            showGlyph={showLandmarks}
            onClick={() => selectDraft(d.id)}
          />
        ))}
        {agents.map((a) => (
          <AgentRow
            key={a.id}
            kind="real"
            agent={a}
            active={a.id === selectedAgentId}
            showGlyph={showLandmarks}
            onClick={() => selectAgent(a.id)}
          />
        ))}
      </div>
    </div>
  );
}
