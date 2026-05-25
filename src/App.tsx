import { useEffect, useState } from "react";
import { WorkspaceBar } from "./components/WorkspaceBar";
import { AgentList } from "./components/AgentList";
import { AgentPanes } from "./components/AgentPanes";
import { ChooseRepoDialog } from "./components/ChooseRepoDialog";
import { useAppStore } from "./store";

export function App() {
  const init = useAppStore((s) => s.init);
  const workspace = useAppStore((s) => s.workspace);
  const spawn = useAppStore((s) => s.spawn);
  const busy = useAppStore((s) => s.busy);

  const [repoOpen, setRepoOpen] = useState(false);

  useEffect(() => {
    init();
  }, [init]);

  async function onSpawn() {
    // Instant spawn — no dialog. Default to the custom (chat) view;
    // the user can flip to native from the agent header. The first
    // user message is sent later via the prompt box.
    await spawn("custom");
  }

  return (
    <div className="app">
      <WorkspaceBar onChooseRepo={() => setRepoOpen(true)} />
      <div className="body">
        <aside className="sidebar">
          <div className="sidebar-header">
            <span className="sidebar-title">Agents</span>
            <button
              className="primary"
              onClick={onSpawn}
              disabled={!workspace || busy}
              title={!workspace ? "Choose a repo first" : ""}
            >
              + Spawn
            </button>
          </div>
          <AgentList />
        </aside>
        <main className="main">
          <AgentPanes />
        </main>
      </div>
      {repoOpen && <ChooseRepoDialog onClose={() => setRepoOpen(false)} />}
    </div>
  );
}
