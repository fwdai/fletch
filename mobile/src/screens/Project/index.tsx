import { Icon } from "@desktop/components/Icon";
import { useEffect, useState } from "react";
import { HostAgentRow } from "../../components/AgentRow/HostAgentRow";
import { Nav, ProjectIcon, Segmented } from "../../components/ui";
import { useHost } from "../../host/useHost";
import { type AgentStage, baseOf, type HostAgent, hostAgentKey, stageOf } from "../../lib/agents";
import { entryOf, repoLabel } from "../../lib/projects";
import {
  useApp,
  useEveryHost,
  useHostKeys,
  useHostNav,
  useHostStore,
  useProjectEntries,
} from "../../store";
import { RoadmapPane } from "./Roadmap";

/** Tab order, which is also the order the screen falls through when picking
 *  the tab to open on. */
const STAGES: AgentStage[] = ["running", "yours", "prs"];

/** The agent stages, then the project's roadmap — never the tab it opens on. */
type Tab = AgentStage | "roadmap";

const NONE: HostAgent[] = [];

/** A project with the agents of every host that has it, each row its own
 *  host's. The rest — the roadmap, the planning chats, starting an agent — is
 *  the host the screen was opened on: the project's speaking host. */
export function ProjectScreen({ projectId }: { projectId: string }) {
  // Derived outside the selectors: a selector that builds a new array on
  // every call re-renders forever under zustand v5.
  const { hostKey } = useHost();
  const entry = entryOf(useProjectEntries(), hostKey, projectId);
  const agents = entry?.agents ?? NONE;
  const hostKeys = useHostKeys();
  const prStates = useEveryHost((s) => s.prStates);
  const pop = useApp((s) => s.pop);
  const { openSheet } = useHostNav();
  // Indexed straight out of the store: the registry's array is a stable
  // reference, where a `?? []` inside the selector would be a fresh one.
  const chats = useHostStore((s) => s.chats[projectId]);
  const loadChats = useHostStore((s) => s.loadChats);
  const connected = useHostStore((s) => s.connection === "connected");
  const canPlan = useHostStore((s) => s.hostSupports("list_project_chats"));
  const hasRoadmap = useHostStore((s) => s.hostSupports("roadmap_list_items"));

  // The live PR state, as the agent's own host has it, wins over the record's
  // persisted snapshot when it is known; most rows only have the snapshot.
  const prStateOf = (h: HostAgent) =>
    prStates[hostKeys.indexOf(h.hostKey)]?.[h.agent.id]?.state ??
    h.agent.repos[0]?.pr_state ??
    null;
  const stage = (h: HostAgent) => stageOf(h.agent, prStateOf(h));
  const count = (s: AgentStage) => agents.filter((h) => stage(h) === s).length;

  // Opens on what is running; when nothing is, on the first tab with something
  // to act on, rather than an empty list. Picked once, so a turn finishing
  // never moves the user off the tab they are reading.
  const [filter, setFilter] = useState<Tab>(() => STAGES.find((s) => count(s) > 0) ?? "running");

  // Planning chats are absent from the workspace snapshot, so they are read on
  // their own — on arrival, and again on every reconnect, since nothing pushes
  // a chat started on the Mac to this screen.
  useEffect(() => {
    if (connected) void loadChats(projectId);
  }, [connected, projectId, loadChats]);

  if (!entry) return null;
  const { project } = entry;

  // Open PRs, still to merge, above the merged and closed ones the hosts'
  // auto-archive is about to clear. The sort is stable, so newest-first holds
  // within each.
  const list = agents
    .filter((h) => stage(h) === filter)
    .sort((a, b) => Number(prStateOf(a) !== "open") - Number(prStateOf(b) !== "open"));

  return (
    <>
      <Nav
        onBack={pop}
        backLabel="Projects"
        title={
          <>
            <ProjectIcon project={project} size={18} />
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
          {agents[0] ? baseOf(agents[0].agent) : "main"}
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
            ...(hasRoadmap ? [{ id: "roadmap", label: "Roadmap" }] : []),
          ]}
          value={filter}
          onChange={(id) => setFilter(id as Tab)}
        />
      </div>
      <div className="scroll proj-body">
        {filter === "roadmap" ? (
          <RoadmapPane projectId={projectId} />
        ) : (
          <div className="card pane" key={filter}>
            {list.map((h) => (
              <HostAgentRow key={hostAgentKey(h)} {...h} />
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
        )}
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
                <HostAgentRow key={c.id} hostKey={hostKey} agent={c} />
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
