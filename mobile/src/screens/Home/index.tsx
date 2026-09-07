import type { ProjectRef } from "@desktop/api/types/agent";
import { AgentRow } from "../../components/AgentRow";
import { Icon } from "../../components/Icon";
import { Swatch } from "../../components/ui";
import { agentsOfProject, baseOf, isActive, isBusy, repoLabel } from "../../lib/agents";
import { useStore } from "../../store";
import { ConnectionBanner } from "./ConnectionBanner";
import { Wordmark } from "./Wordmark";

function ProjectCard({ project }: { project: ProjectRef }) {
  // Selectors must return a stable reference — a fresh array on every call
  // re-renders forever under zustand v5's useSyncExternalStore. So the
  // workspace comes out of the store as-is and is filtered here.
  const workspace = useStore((s) => s.workspace);
  const agents = agentsOfProject(workspace, project.project_id);
  const openAgent = useStore((s) => s.openAgent);
  const push = useStore((s) => s.push);
  const active = agents.filter(isActive);
  const running = agents.filter(isBusy).length;
  const rest = agents.length - active.length;
  return (
    <div
      className="pc"
      role="button"
      tabIndex={0}
      onClick={() => push("project", { projectId: project.project_id })}
      onKeyDown={(e) => {
        if (e.key === "Enter") push("project", { projectId: project.project_id });
      }}
    >
      <div className="pc-h">
        <Swatch project={project} size={32} />
        <div>
          <div className="nm">{project.name}</div>
          <div className="rp">
            <Icon name="github" size={11} />
            {repoLabel(project)}
            <span>·</span>
            <Icon name="branch" size={11} />
            {agents[0] ? baseOf(agents[0]) : "main"}
          </div>
        </div>
        <div className="cnt">
          {running > 0 ? (
            <>
              <b>{running} running</b>
              <span>· {agents.length}</span>
            </>
          ) : (
            <span>
              {agents.length} agent{agents.length === 1 ? "" : "s"}
            </span>
          )}
          <Icon name="chevR" size={16} className="chev" />
        </div>
      </div>
      {active.length > 0 && (
        <div className="pc-list">
          {active.slice(0, 3).map((a) => (
            <AgentRow
              key={a.id}
              agent={a}
              project={project}
              // The whole card is clickable too; the row wins.
              onClick={(e) => {
                e.stopPropagation();
                openAgent(a.id);
              }}
            />
          ))}
        </div>
      )}
      {rest > 0 && (
        <div className="pc-more">
          {rest} more {rest === 1 ? "agent" : "agents"}
        </div>
      )}
      {agents.length === 0 && (
        <div className="pc-more" style={{ paddingLeft: 14 }}>
          No agents yet — start one
        </div>
      )}
    </div>
  );
}

export function HomeScreen() {
  const workspace = useStore((s) => s.workspace);
  const projects = workspace?.projects ?? [];
  const agents = workspace?.agents ?? [];
  const pendingTotal = useStore((s) =>
    Object.values(s.pendingToolUse).reduce((n, m) => n + Object.keys(m).length, 0),
  );
  const openSheet = useStore((s) => s.openSheet);
  const hostName = useStore((s) => s.hostInfo?.name);
  const connection = useStore((s) => s.connection);
  const errored = agents.filter((a) => a.status === "error").length;
  const running = agents.filter(isBusy).length;
  const attention = pendingTotal + errored;

  return (
    <>
      <div className="home-head">
        <Wordmark />
        <button type="button" className="host" onClick={() => openSheet("host")}>
          <span className={`dot ${connection === "connected" ? "running" : "error"}`} />
          {hostName ?? "Host"}
        </button>
      </div>
      <ConnectionBanner />
      <div className="scroll home-body">
        <div className="sect">
          Projects <span className="n">{projects.length}</span>
          <span className="grow" />
          {attention > 0 && (
            <span
              className="n"
              style={{
                color: "var(--warn)",
                textTransform: "none",
                letterSpacing: 0,
                display: "inline-flex",
                alignItems: "center",
                gap: 4,
              }}
            >
              <Icon name="hand" size={11} sw={1.8} />
              {attention}
            </span>
          )}
          {running > 0 && (
            <span
              className="n"
              style={{ color: "var(--success)", textTransform: "none", letterSpacing: 0 }}
            >
              {running} running
            </span>
          )}
        </div>
        {projects.map((p) => (
          <ProjectCard key={p.project_id} project={p} />
        ))}
        {projects.length === 0 && (
          <div className="empty">
            <b>No projects on the host</b>
            Pin a repo in Fletch on your Mac and it shows up here.
          </div>
        )}
      </div>
      <div className="fab-wrap">
        <button type="button" className="btn primary" onClick={() => openSheet("newAgent")}>
          <Icon name="plus" size={18} sw={2.2} />
          New agent
        </button>
      </div>
    </>
  );
}
