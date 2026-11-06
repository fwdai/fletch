import type { ReactNode } from "react";
import { ProjectIcon } from "@/components/ProjectIcon";
import { VIEW_LABEL } from "@/components/ProjectScreen/Header";
import { useAppStore } from "@/store";
import { projectView } from "@/store/surface";
import { basename } from "@/util/format";
import { Capsule } from "./Capsule";
import { ProjectPill } from "./ProjectPill";
import { StatusDot } from "./StatusDot";

/** The center of the title bar. Adapts to context: the active agent's live
 *  status capsule, a draft's pending name, a quiet fleet summary at Home, or a
 *  plain crumb in the settings / usage / project views. Replaces the old repo/agent
 *  breadcrumb. */
export function WorkspaceStatus() {
  const settingsScreenOpen = useAppStore((s) => s.settingsScreenOpen);
  const usageScreenOpen = useAppStore((s) => s.usageScreenOpen);
  const projectRepoPath = useAppStore((s) => s.projectScreen?.repoPath ?? null);
  const view = useAppStore(projectView);
  const workspace = useAppStore((s) => s.workspace);
  const selectedId = useAppStore((s) => s.selectedAgentId);
  const drafts = useAppStore((s) => s.drafts);
  const activeDraftId = useAppStore((s) => s.activeDraftId);

  // Project display name for a repo path: the user-editable project name
  // (as in the sidebar), falling back to the folder basename.
  const projectRef = (repoPath: string) => workspace?.projects.find((p) => p.path === repoPath);
  const projectName = (repoPath: string) => projectRef(repoPath)?.name ?? basename(repoPath);

  if (settingsScreenOpen) return <ScreenCrumb label="Settings" />;
  if (usageScreenOpen) return <ScreenCrumb label="Usage" />;
  // A project view sits a level above any workspace: the same crumb, with
  // the project's mark and the view's name.
  if (projectRepoPath && view) {
    const name = projectName(projectRepoPath);
    return (
      <ScreenCrumb
        label={name}
        icon={<ProjectIcon project={projectRef(projectRepoPath) ?? { name }} size={12} />}
        view={VIEW_LABEL[view]}
      />
    );
  }

  const draft = activeDraftId ? drafts.find((d) => d.id === activeDraftId) : null;
  const agent = !draft && selectedId ? workspace?.agents.find((a) => a.id === selectedId) : null;

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
      <span className="ws-home-title">Home</span>
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

/** A full-screen surface (settings, usage, a project view) — a plain crumb
 *  naming it, ending on the view's name when there is one. The way back is the
 *  screen's own back button. */
function ScreenCrumb({ label, icon, view }: { label: string; icon?: ReactNode; view?: string }) {
  return (
    <div className="ws-plain">
      {icon}
      <span className={view ? undefined : "ws-plain-active"}>{label}</span>
      {view && (
        <>
          <span className="ws-plain-sep">/</span>
          <span className="ws-plain-active">{view}</span>
        </>
      )}
    </div>
  );
}

const EMPTY: never[] = [];
