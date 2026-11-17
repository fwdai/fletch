import { useCallback, useEffect } from "react";
import { Stack } from "./components/Stack";
import { OnHost } from "./host/OnHost";
import { usePoll, useSystemTheme } from "./lib/hooks";
import { AgentScreen } from "./screens/Agent";
import { CodeScreen } from "./screens/Code";
import { DiffScreen } from "./screens/Diff";
import { FileScreen } from "./screens/File";
import { HomeScreen } from "./screens/Home";
import { PairScreen } from "./screens/Pair";
import { ProjectScreen } from "./screens/Project";
import { ProvidersScreen } from "./screens/Providers";
import { SubagentScreen } from "./screens/Subagent";
import { AccountPickerSheet } from "./sheets/AccountPickerSheet";
import { AddProjectSheet } from "./sheets/AddProjectSheet";
import { AgentMoreSheet } from "./sheets/AgentMoreSheet";
import { HostSheet } from "./sheets/HostSheet";
import { ModelPickerSheet } from "./sheets/ModelPickerSheet";
import { NewAgentSheet } from "./sheets/NewAgentSheet";
import { NewPlanSheet } from "./sheets/NewPlanSheet";
import { PrSheet } from "./sheets/PrSheet";
import { ShipMoreSheet } from "./sheets/ShipMoreSheet";
import { type NavItem, useApp, useHostStore } from "./store";
import "./styles/base.css";
import "./styles/screens.css";
import "@desktop/components/ui/Loader.css";
import "@desktop/components/ProjectIcon/ProjectIcon.css";
import "@desktop/styles/shared/icon-button.css";
import "@desktop/components/Workspace/messages/Steps.css";
import "./styles/agent.css";

/** Sheets that push the whole stack back rather than sitting over it. */
const FULL_SHEETS = new Set(["newAgent", "newPlan", "addProject", "pr"]);

function Screen({ item }: { item: NavItem }) {
  switch (item.screen) {
    case "home":
      return <HomeScreen />;
    case "project":
      return <ProjectScreen projectId={item.props.projectId} />;
    case "agent":
      return <AgentScreen agentId={item.props.agentId} tab={item.props.tab} />;
    case "subagent":
      return <SubagentScreen agentId={item.props.agentId} path={item.props.path} />;
    case "code":
      return <CodeScreen agentId={item.props.agentId} />;
    case "file":
      return <FileScreen agentId={item.props.agentId} path={item.props.path} />;
    case "diff":
      return <DiffScreen agentId={item.props.agentId} path={item.props.path} />;
    case "providers":
      return <ProvidersScreen />;
    default:
      return null;
  }
}

/** One fleet-wide poll for the host on screen, like the desktop's — every agent
 *  row on every screen reads the maps these fill, so no row polls for itself.
 *  Working-tree stats are local git on the host, with no event for them, so
 *  they poll. PR state does not: it is seeded on every handshake and return to
 *  the foreground (store/index) and the host watcher's `pr:*` events keep it
 *  current between. */
function ShortstatsPoll() {
  const connected = useHostStore((s) => s.connection === "connected");
  const loadShortstats = useHostStore((s) => s.loadShortstats);
  usePoll(loadShortstats, 10_000, connected);
  return null;
}

export function App() {
  const ready = useApp((s) => s.ready);
  const init = useApp((s) => s.init);
  const theme = useApp((s) => s.theme);
  const systemTheme = useApp((s) => s.systemTheme);
  const setSystemTheme = useApp((s) => s.setSystemTheme);
  const sheet = useApp((s) => s.sheet);
  const closeSheet = useApp((s) => s.closeSheet);
  // Paired means "we hold the host's key": there is no other credential. The
  // Pair screen also comes up over a paired host, to add another.
  const hostKey = useApp((s) => s.hostKey);
  const addingHost = useApp((s) => s.addingHost);

  useEffect(() => {
    void init();
  }, [init]);
  useSystemTheme(useCallback((t) => setSystemTheme(t), [setSystemTheme]));

  const resolved = theme === "system" ? systemTheme : theme;
  const props = (name: string) => (sheet?.name === name ? sheet.props : {});
  const isOpen = (name: string) => !!(sheet?.name === name && sheet.open);
  const dimmed = !!sheet?.open && FULL_SHEETS.has(sheet.name);
  /** The host a sheet acts on, as its props name it; one that names none acts
   *  on the host on screen. */
  const on = (name: string) => props(name).hostKey;

  return (
    <div className={`m-app theme-${resolved}`}>
      {!ready ? null : hostKey && !addingHost ? (
        <>
          <OnHost hostKey={hostKey}>
            <ShortstatsPoll />
          </OnHost>
          <Stack
            dimmed={dimmed}
            render={(item) => (
              <OnHost hostKey={item.props.hostKey ?? hostKey}>
                <Screen item={item} />
              </OnHost>
            )}
          />
          <OnHost hostKey={on("host") ?? hostKey}>
            <HostSheet open={isOpen("host")} onClose={closeSheet} />
          </OnHost>
          <OnHost hostKey={on("newAgent") ?? hostKey}>
            <NewAgentSheet open={isOpen("newAgent")} onClose={closeSheet} {...props("newAgent")} />
          </OnHost>
          <OnHost hostKey={on("newPlan") ?? hostKey}>
            <NewPlanSheet open={isOpen("newPlan")} onClose={closeSheet} {...props("newPlan")} />
          </OnHost>
          <OnHost hostKey={on("addProject") ?? hostKey}>
            <AddProjectSheet
              open={isOpen("addProject")}
              onClose={closeSheet}
              {...props("addProject")}
            />
          </OnHost>
          <OnHost hostKey={on("agentMore") ?? hostKey}>
            <AgentMoreSheet
              open={isOpen("agentMore")}
              onClose={closeSheet}
              {...props("agentMore")}
            />
          </OnHost>
          <OnHost hostKey={on("shipMore") ?? hostKey}>
            <ShipMoreSheet open={isOpen("shipMore")} onClose={closeSheet} {...props("shipMore")} />
          </OnHost>
          <OnHost hostKey={on("modelPicker") ?? hostKey}>
            <ModelPickerSheet
              open={isOpen("modelPicker")}
              onClose={closeSheet}
              {...props("modelPicker")}
            />
          </OnHost>
          <OnHost hostKey={on("accountPicker") ?? hostKey}>
            <AccountPickerSheet
              open={isOpen("accountPicker")}
              onClose={closeSheet}
              {...props("accountPicker")}
            />
          </OnHost>
          <OnHost hostKey={on("pr") ?? hostKey}>
            <PrSheet open={isOpen("pr")} onClose={closeSheet} {...props("pr")} />
          </OnHost>
        </>
      ) : (
        <PairScreen />
      )}
    </div>
  );
}
