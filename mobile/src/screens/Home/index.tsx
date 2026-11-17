import { Icon } from "@desktop/components/Icon";
import { OnHost } from "../../host/OnHost";
import { useOrderedProjects } from "../../lib/activeProjects";
import {
  severalHosts,
  useApp,
  useEveryHost,
  useHostKeys,
  useHostNav,
  useHostStore,
} from "../../store";
import { AutoArchivedNotice } from "./AutoArchivedNotice";
import { ConnectionBanner } from "./ConnectionBanner";
import { EmptyProjects } from "./EmptyProjects";
import { HostState } from "./HostState";
import { HostSwitcher } from "./HostSwitcher";
import { ProjectCard } from "./ProjectCard";
import { ProjectsHead } from "./ProjectsHead";
import { Wordmark } from "./Wordmark";

/** Every paired host's projects in one list. */
export function HomeScreen() {
  const projects = useOrderedProjects();
  const hostKeys = useHostKeys();
  const workspaces = useEveryHost((s) => s.workspace);
  const connections = useEveryHost((s) => s.connection);
  // Planning chats are listed by their own op; a Mac without it offers nothing
  // new (docs/remote-protocol.md, "Compatibility").
  const canPlan = useEveryHost((s) => s.hostSupports("list_project_chats")).includes(true);
  const several = useApp(severalHosts);
  const { openSheet, push } = useHostNav();
  // The one paired host's provider accounts, a tap from the host chip — on a
  // Mac whose Fletch can list them; an older one shows the chip alone. With
  // several hosts, each one's are on its Host sheet instead.
  const providers = useHostStore((s) => s.hostSupports("list_provider_accounts"));
  const hasProviders = providers && !several;
  const connected = connections.includes("connected");
  // What the body is: the list when there is one to show, the connection state
  // when no host has anything behind it. A cached list survives a dropped link
  // (read-only, with the status line over it); an empty one does not — "no
  // projects" is only true of a host that has answered.
  const showList =
    projects.length > 0 || workspaces.some((w, i) => w !== null && connections[i] === "connected");
  const body = !showList ? (
    <HostState />
  ) : projects.length === 0 ? (
    <EmptyProjects />
  ) : (
    projects.map((p) => <ProjectCard key={p.key} entry={p} />)
  );

  return (
    <>
      <div className="home-head">
        <Wordmark />
        <div className={`host-pair${hasProviders ? "" : " alone"}`}>
          <HostSwitcher />
          {hasProviders && (
            <button
              type="button"
              className="host-cog"
              aria-label="Providers and accounts"
              onClick={() => push("providers")}
            >
              <Icon name="settings" size={15} strokeWidth={1.9} />
            </button>
          )}
        </div>
      </div>
      {showList && <ConnectionBanner />}
      {showList &&
        hostKeys.map((hostKey) => (
          <OnHost key={hostKey} hostKey={hostKey}>
            <AutoArchivedNotice />
          </OnHost>
        ))}
      <div className={`scroll home-body${showList && projects.length > 0 ? "" : " is-blank"}`}>
        {showList && projects.length > 0 && (
          <ProjectsHead count={projects.length} canAdd={connected} />
        )}
        {body}
      </div>
      {/* Starting an agent needs a live host and a project to put it in. */}
      {showList && projects.length > 0 && connected && (
        <div className={`fab-wrap${canPlan ? " duo" : ""}`}>
          <button type="button" className="btn primary" onClick={() => openSheet("newAgent")}>
            <Icon name="plus" size={18} strokeWidth={2.2} />
            New agent
          </button>
          {canPlan && (
            <button type="button" className="btn ghost" onClick={() => openSheet("newPlan")}>
              <Icon name="map" size={18} strokeWidth={2.2} />
              Plan with PM
            </button>
          )}
        </div>
      )}
    </>
  );
}
