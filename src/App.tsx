import { useEffect, useState } from "react";
import { WorkspaceBar } from "./components/WorkspaceBar";
import { AgentList } from "./components/AgentList";
import { AgentPanes } from "./components/AgentPanes";
import { SpawnDialog } from "./components/SpawnDialog";
import { ChooseRepoDialog } from "./components/ChooseRepoDialog";
import { useAppStore } from "./store";

export function App() {
  const init = useAppStore((s) => s.init);
  const workspace = useAppStore((s) => s.workspace);

  const [spawnOpen, setSpawnOpen] = useState(false);
  const [repoOpen, setRepoOpen] = useState(false);

  useEffect(() => {
    init();
  }, [init]);

  return (
    <div className="app">
      <WorkspaceBar onChooseRepo={() => setRepoOpen(true)} />
      <div className="body">
        <aside className="sidebar">
          <div className="sidebar-header">
            <span className="sidebar-title">Agents</span>
            <button
              className="primary"
              onClick={() => setSpawnOpen(true)}
              disabled={!workspace}
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
      {spawnOpen && <SpawnDialog onClose={() => setSpawnOpen(false)} />}
      {repoOpen && <ChooseRepoDialog onClose={() => setRepoOpen(false)} />}
    </div>
  );
}
