import { useCallback, useEffect } from "react";
import { Stack } from "./components/Stack";
import { useSystemTheme } from "./lib/hooks";
import { AgentScreen } from "./screens/Agent";
import { DiffScreen } from "./screens/Diff";
import { FileScreen } from "./screens/File";
import { HomeScreen } from "./screens/Home";
import { PairScreen } from "./screens/Pair";
import { ProjectScreen } from "./screens/Project";
import { AddProjectSheet } from "./sheets/AddProjectSheet";
import { AgentMoreSheet } from "./sheets/AgentMoreSheet";
import { HostSheet } from "./sheets/HostSheet";
import { ModelPickerSheet } from "./sheets/ModelPickerSheet";
import { NewAgentSheet } from "./sheets/NewAgentSheet";
import { PrSheet } from "./sheets/PrSheet";
import { type NavItem, useStore } from "./store";
import "./styles/base.css";
import "./styles/screens.css";
import "./styles/agent.css";

/** Sheets that push the whole stack back rather than sitting over it. */
const FULL_SHEETS = new Set(["newAgent", "addProject", "pr"]);

function Screen({ item }: { item: NavItem }) {
  switch (item.screen) {
    case "home":
      return <HomeScreen />;
    case "project":
      return <ProjectScreen projectId={item.props.projectId} />;
    case "agent":
      return <AgentScreen agentId={item.props.agentId} />;
    case "file":
      return <FileScreen agentId={item.props.agentId} path={item.props.path} />;
    case "diff":
      return <DiffScreen agentId={item.props.agentId} path={item.props.path} />;
    default:
      return null;
  }
}

export function App() {
  const ready = useStore((s) => s.ready);
  const init = useStore((s) => s.init);
  const theme = useStore((s) => s.theme);
  const systemTheme = useStore((s) => s.systemTheme);
  const setSystemTheme = useStore((s) => s.setSystemTheme);
  const sheet = useStore((s) => s.sheet);
  const closeSheet = useStore((s) => s.closeSheet);
  // Paired means "we hold the host's key": there is no other credential.
  const hostKey = useStore((s) => s.hostKey);

  useEffect(() => {
    void init();
  }, [init]);
  useSystemTheme(useCallback((t) => setSystemTheme(t), [setSystemTheme]));

  const resolved = theme === "system" ? systemTheme : theme;
  const props = (name: string) => (sheet?.name === name ? sheet.props : {});
  const isOpen = (name: string) => !!(sheet?.name === name && sheet.open);
  const dimmed = !!sheet?.open && FULL_SHEETS.has(sheet.name);

  return (
    <div className={`m-app theme-${resolved}`}>
      {!ready ? null : hostKey ? (
        <>
          <Stack dimmed={dimmed} render={(item) => <Screen item={item} />} />
          <HostSheet open={isOpen("host")} onClose={closeSheet} />
          <NewAgentSheet open={isOpen("newAgent")} onClose={closeSheet} {...props("newAgent")} />
          <AddProjectSheet
            open={isOpen("addProject")}
            onClose={closeSheet}
            {...props("addProject")}
          />
          <AgentMoreSheet open={isOpen("agentMore")} onClose={closeSheet} {...props("agentMore")} />
          <ModelPickerSheet
            open={isOpen("modelPicker")}
            onClose={closeSheet}
            {...props("modelPicker")}
          />
          <PrSheet open={isOpen("pr")} onClose={closeSheet} {...props("pr")} />
        </>
      ) : (
        <PairScreen />
      )}
    </div>
  );
}
