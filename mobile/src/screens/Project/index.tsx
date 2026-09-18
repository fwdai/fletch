import { Icon } from "@desktop/components/Icon";
import { useEffect, useState } from "react";
import { AgentRow } from "../../components/AgentRow";
import { Nav, Segmented, Swatch } from "../../components/ui";
import { agentsOfProject, baseOf, isActive, isBusy, repoLabel } from "../../lib/agents";
import { useStore } from "../../store";

type Filter = "all" | "active" | "prs";

export function ProjectScreen({ projectId }: { projectId: string }) {
  // Derived outside the selector: a selector that builds a new array on every
  // call re-renders forever under zustand v5.
  const workspace = useStore((s) => s.workspace);
  const project = workspace?.projects.find((p) => p.project_id === projectId);
  const agents = agentsOfProject(workspace, projectId);
  const prStates = useStore((s) => s.prStates);
  const pop = useStore((s) => s.pop);
  const openAgent = useStore((s) => s.openAgent);
  const openSheet = useStore((s) => s.openSheet);
  // Indexed straight out of the store: the registry's array is a stable
  // reference, where a `?? []` inside the selector would be a fresh one.
  const chats = useStore((s) => s.chats[projectId]);
  const loadChats = useStore((s) => s.loadChats);
  const connected = useStore((s) => s.connection === "connected");
  const canPlan = useStore((s) => s.hostSupports("list_project_chats"));
  const [filter, setFilter] = useState<Filter>("all");

  // Planning chats are absent from the workspace snapshot, so they are read on
  // their own — on arrival, and again on every reconnect, since nothing pushes
  // a chat started on the Mac to this screen.
  useEffect(() => {
    if (connected) void loadChats(projectId);
  }, [connected, projectId, loadChats]);

  if (!project) return null;

  const hasPr = (id: string) =>
    !!prStates[id] || !!agents.find((a) => a.id === id)?.repos[0]?.pr_number;
  const list = agents.filter((a) =>
    filter === "all" ? true : filter === "active" ? isActive(a) : hasPr(a.id),
  );
  const running = agents.filter(isBusy).length;
  const prs = agents.filter((a) => hasPr(a.id)).length;

  return (
    <>
      <Nav
        onBack={pop}
        backLabel="Projects"
        title={
          <>
            <Swatch project={project} size={18} />
            {project.name}
          </>
        }
        sub={repoLabel(project)}
        right={
          <button
            type="button"
            className="ibtn"
            onClick={() => openSheet("newAgent", { projectId })}
            aria-label="New agent"
          >
            <Icon name="plus" size={20} strokeWidth={1.8} />
          </button>
        }
      />
      <div className="proj-sum">
        <span className="pill mono">
          <Icon name="branch" size={11} />
          {agents[0] ? baseOf(agents[0]) : "main"}
        </span>
        <span className="pill mono">
          {agents.length} agent{agents.length === 1 ? "" : "s"}
        </span>
        {running > 0 && (
          <span className="pill mono ok">
            <span className="dot running" style={{ width: 6, height: 6 }} />
            {running} running
          </span>
        )}
        {prs > 0 && (
          <span className="pill mono">
            <Icon name="pr" size={11} />
            {prs} PR{prs > 1 ? "s" : ""}
          </span>
        )}
      </div>
      <div className="proj-tabs">
        <Segmented
          items={[
            { id: "all", label: "All" },
            { id: "active", label: "Active", count: agents.filter(isActive).length },
            { id: "prs", label: "With PR" },
          ]}
          value={filter}
          onChange={(id) => setFilter(id as Filter)}
        />
      </div>
      <div className="scroll proj-body">
        <div className="card pane" key={filter}>
          {list.map((a) => (
            <AgentRow key={a.id} agent={a} project={project} onClick={() => openAgent(a.id)} />
          ))}
          {list.length === 0 && (
            <div className="empty">
              <b>Nothing here</b>
              No agents match this filter.
            </div>
          )}
        </div>
        {/* Below the filtered agents, and outside them: a planning chat is not
            a filter of the fleet, it is a conversation about what the fleet
            should build next. */}
        {canPlan && chats && chats.length > 0 && (
          <>
            <div className="sect">
              Planning <span className="n">{chats.length}</span>
            </div>
            <div className="card">
              {chats.map((c) => (
                <AgentRow key={c.id} agent={c} project={project} onClick={() => openAgent(c.id)} />
              ))}
            </div>
          </>
        )}
      </div>
      <div className={`fab-wrap${canPlan ? " duo" : ""}`}>
        <button
          type="button"
          className="btn primary"
          onClick={() => openSheet("newAgent", { projectId })}
        >
          <Icon name="plus" size={18} strokeWidth={2.2} />
          {canPlan ? "New agent" : `New agent in ${project.name}`}
        </button>
        {canPlan && (
          <button
            type="button"
            className="btn ghost"
            onClick={() => openSheet("newPlan", { projectId })}
          >
            <Icon name="map" size={18} strokeWidth={2.2} />
            Plan with PM
          </button>
        )}
      </div>
    </>
  );
}
