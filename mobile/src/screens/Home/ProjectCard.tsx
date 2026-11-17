import { Icon } from "@desktop/components/Icon";
import { HostAgentRow } from "../../components/AgentRow/HostAgentRow";
import { ProjectIcon } from "../../components/ui";
import { OnHost } from "../../host/OnHost";
import { baseOf, hostAgentKey, isActive, isBusy } from "../../lib/agents";
import { type ProjectEntry, repoLabel } from "../../lib/projects";
import { useHostNav } from "../../store";

/** A project on Home, with the agents of every host that has it. Its own
 *  parts — name, icon, repo label, and the Project screen it opens — are its
 *  speaking host's; each agent row is its own host's. */
export function ProjectCard({ entry }: { entry: ProjectEntry }) {
  return (
    <OnHost hostKey={entry.hostKey}>
      <Card entry={entry} />
    </OnHost>
  );
}

function Card({ entry: { project, agents } }: { entry: ProjectEntry }) {
  const { push } = useHostNav();
  const active = agents.filter((h) => isActive(h.agent));
  const running = agents.filter((h) => isBusy(h.agent)).length;
  const rest = agents.length - active.length;
  const open = () => push("project", { projectId: project.id });
  return (
    <div
      className="pc"
      role="button"
      tabIndex={0}
      onClick={open}
      onKeyDown={(e) => {
        if (e.key === "Enter") open();
      }}
    >
      <div className="pc-h">
        <ProjectIcon project={project} size={32} />
        <div>
          <div className="nm">{project.name}</div>
          <div className="rp">
            <Icon name="github" size={11} />
            {repoLabel(project)}
            <span>·</span>
            <Icon name="branch" size={11} />
            {agents[0] ? baseOf(agents[0].agent) : "main"}
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
          {/* The whole card is clickable too; the row wins. */}
          {active.slice(0, 3).map((h) => (
            <HostAgentRow key={hostAgentKey(h)} {...h} />
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
