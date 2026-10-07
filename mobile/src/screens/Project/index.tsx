import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import { useEffect, useState } from "react";
import { AgentRow } from "../../components/AgentRow";
import { Nav, Segmented, Swatch } from "../../components/ui";
import { type AgentStage, agentsOfProject, baseOf, stageOf } from "../../lib/agents";
import { projectById, repoLabel } from "../../lib/projects";
import { useStore } from "../../store";

/** Tab order, which is also the order the screen falls through when picking
 *  the tab to open on. */
const STAGES: AgentStage[] = ["running", "yours", "prs"];

export function ProjectScreen({ projectId }: { projectId: string }) {
  // Derived outside the selector: a selector that builds a new array on every
  // call re-renders forever under zustand v5.
  const workspace = useStore((s) => s.workspace);
  const project = projectById(workspace, projectId);
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

  // The live PR state wins over the record's persisted snapshot when it is
  // known; most rows only have the snapshot.
  const prStateOf = (a: AgentRecord) => prStates[a.id]?.state ?? a.repos[0]?.pr_state ?? null;
  const stage = (a: AgentRecord) => stageOf(a, prStateOf(a));
  const count = (s: AgentStage) => agents.filter((a) => stage(a) === s).length;

  // Opens on what is running; when nothing is, on the first tab with something
  // to act on, rather than an empty list. Picked once, so a turn finishing
  // never moves the user off the tab they are reading.
  const [filter, setFilter] = useState<AgentStage>(
    () => STAGES.find((s) => count(s) > 0) ?? "running",
  );

  // Planning chats are absent from the workspace snapshot, so they are read on
  // their own — on arrival, and again on every reconnect, since nothing pushes
  // a chat started on the Mac to this screen.
  useEffect(() => {
    if (connected) void loadChats(projectId);
  }, [connected, projectId, loadChats]);

  if (!project) return null;

  // Open PRs, still to merge, above the merged and closed ones the host's
  // auto-archive is about to clear. The sort is stable, so newest-first holds
  // within each.
  const list = agents
    .filter((a) => stage(a) === filter)
    .sort((a, b) => Number(prStateOf(a) !== "open") - Number(prStateOf(b) !== "open"));

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
      </div>
      <div className="proj-tabs">
        <Segmented
          items={[
            { id: "running", label: "Running", count: count("running") },
            { id: "yours", label: "Your turn", count: count("yours") },
            { id: "prs", label: "PRs", count: count("prs") },
          ]}
          value={filter}
          onChange={(id) => setFilter(id as AgentStage)}
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
              {filter === "running"
                ? "No agents are working right now."
                : filter === "yours"
                  ? "No agent is waiting on you."
                  : "No PRs to land."}
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
