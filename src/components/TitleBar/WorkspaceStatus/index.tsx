import { useAppStore } from "@/store";
import { basename } from "@/util/format";
import { Capsule } from "./Capsule";
import { StatusDot } from "./StatusDot";

/** The center of the title bar. Adapts to context: the active agent's live
 *  status capsule, a draft's pending name, a quiet fleet summary at Home, or a
 *  plain crumb in the settings screen. Replaces the old repo/agent breadcrumb. */
export function WorkspaceStatus() {
  const settingsScreenOpen = useAppStore((s) => s.settingsScreenOpen);
  const closeSettingsScreen = useAppStore((s) => s.closeSettingsScreen);
  const workspace = useAppStore((s) => s.workspace);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const drafts = useAppStore((s) => s.drafts);
  const activeDraftId = useAppStore((s) => s.activeDraftId);

  if (settingsScreenOpen) return <SettingsCrumb onHome={closeSettingsScreen} />;

  const draft = activeDraftId ? drafts.find((d) => d.id === activeDraftId) : null;
  if (draft) return <DraftCapsule name={draft.name} repoPath={draft.repoPath} />;

  const agent = selectedId ? workspace?.agents.find((a) => a.id === selectedId) : null;
  if (agent) {
    const repoPath = agent.repos[0]?.repo_path;
    return <Capsule agent={agent} projectName={repoPath ? basename(repoPath) : agent.name} />;
  }

  return <HomeSummary />;
}

/** Home — a quiet count of what's working and what needs you across the fleet. */
function HomeSummary() {
  const agents = useAppStore((s) => s.workspace?.agents ?? EMPTY);
  const pending = useAppStore((s) => s.pendingToolUse);
  const working = agents.filter((a) => a.status === "running" || a.status === "spawning");
  const waiting = working.filter((a) => Object.keys(pending[a.id] ?? {}).length > 0).length;
  const run = working.length - waiting;

  return (
    <div className="ws-plain home">
      <span className="ws-home-title">fletch</span>
      {(run > 0 || waiting > 0) && <span className="ws-plain-sep">·</span>}
      {run > 0 && (
        <span className="ws-home-stat">
          <StatusDot status="running" />
          {run} working
        </span>
      )}
      {waiting > 0 && (
        <span className="ws-home-stat">
          <StatusDot status="waiting" />
          {waiting} waiting
        </span>
      )}
      {run === 0 && waiting === 0 && <span className="ws-plain-active">All workspaces</span>}
    </div>
  );
}

/** A not-yet-spawned draft: idle dot, project/name, a quiet "new agent" tag. */
function DraftCapsule({ name, repoPath }: { name: string; repoPath: string | null }) {
  return (
    <div className="ws-cap-wrap">
      <div className="ws-cap static">
        <span className="ws-ctx">
          <StatusDot status="idle" />
          {repoPath && <span className="ws-proj-name">{basename(repoPath)}</span>}
          {repoPath && <span className="ws-slash">/</span>}
          <span className="ws-agent-name">{name}</span>
        </span>
        <span className="ws-badge quiet mono">new agent</span>
      </div>
    </div>
  );
}

/** Settings screen — a plain crumb back to Home. */
function SettingsCrumb({ onHome }: { onHome: () => void }) {
  return (
    <div className="ws-plain">
      <button type="button" className="ws-plain-btn" onClick={onHome}>
        fletch
      </button>
      <span className="ws-plain-sep">/</span>
      <span className="ws-plain-active">Settings</span>
    </div>
  );
}

const EMPTY: never[] = [];
