import { useAppStore } from "@/store";
import { basename } from "@/util/format";
import { Capsule } from "./Capsule";
import { agentDotStatus } from "./derive";
import { ProjectPill } from "./ProjectPill";
import { StatusDot } from "./StatusDot";

/** The center of the title bar. Adapts to context: the active agent's live
 *  status capsule, a draft's pending name, a quiet fleet summary at Home, or a
 *  plain crumb in the settings / project screens. Replaces the old repo/agent
 *  breadcrumb. */
export function WorkspaceStatus() {
  const settingsScreenOpen = useAppStore((s) => s.settingsScreenOpen);
  const closeSettingsScreen = useAppStore((s) => s.closeSettingsScreen);
  const projectScreenRepoPath = useAppStore((s) => s.projectScreenRepoPath);
  const workspace = useAppStore((s) => s.workspace);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const drafts = useAppStore((s) => s.drafts);
  const activeDraftId = useAppStore((s) => s.activeDraftId);
  const pending = useAppStore((s) => s.pendingToolUse);

  // Project display name for a repo path: the user-editable project name
  // (as in the sidebar), falling back to the folder basename.
  const projectName = (repoPath: string) =>
    workspace?.projects.find((p) => p.path === repoPath)?.name ?? basename(repoPath);

  if (settingsScreenOpen) return <SettingsCrumb onHome={closeSettingsScreen} />;

  const draft = activeDraftId ? drafts.find((d) => d.id === activeDraftId) : null;
  const agent = !draft && selectedId ? workspace?.agents.find((a) => a.id === selectedId) : null;

  // On the project page the user sits a level above any workspace, so no
  // workspace pill — just marker + project name, plain. The marker keeps the
  // capsule's display logic: the selected agent's live status when it belongs
  // to this project; a draft (or no relevant selection) reads idle.
  if (projectScreenRepoPath) {
    const onProject = agent?.repos[0]?.repo_path === projectScreenRepoPath;
    const status = agent && onProject ? agentDotStatus(agent.status, pending[agent.id]) : "idle";
    return (
      <div className="ws-plain">
        <StatusDot status={status} />
        <span className="ws-plain-active">{projectName(projectScreenRepoPath)}</span>
      </div>
    );
  }

  if (draft) {
    return (
      <DraftCapsule
        name={draft.name}
        repoPath={draft.repoPath}
        projectName={draft.repoPath ? projectName(draft.repoPath) : null}
      />
    );
  }

  if (agent) {
    const repoPath = agent.repos[0]?.repo_path ?? null;
    return (
      <Capsule
        agent={agent}
        repoPath={repoPath}
        projectName={repoPath ? projectName(repoPath) : null}
      />
    );
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

/** A not-yet-spawned draft: the project pill, then a static pill with the
 *  idle dot, pending name, and a quiet "new agent" tag. */
function DraftCapsule({
  name,
  repoPath,
  projectName,
}: {
  name: string;
  repoPath: string | null;
  projectName: string | null;
}) {
  return (
    <div className="ws-cap-wrap">
      {repoPath && projectName && (
        <ProjectPill repoPath={repoPath} name={projectName} status="idle" />
      )}
      <div className="ws-cap static">
        <span className="ws-ctx">
          {!(repoPath && projectName) && <StatusDot status="idle" />}
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
