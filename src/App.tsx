import { useEffect, useState } from "react";
import { WorkspaceBar } from "./components/WorkspaceBar";
import { AgentList } from "./components/AgentList";
import { AgentPanes } from "./components/AgentPanes";
import { SpawnDialog } from "./components/SpawnDialog";
import { ChooseRepoDialog } from "./components/ChooseRepoDialog";
import { BakeDialog } from "./components/BakeDialog";
import { MissingBaseImageBanner } from "./components/MissingBaseImageBanner";
import { useAppStore } from "./store";

export function App() {
  const init = useAppStore((s) => s.init);
  const workspace = useAppStore((s) => s.workspace);
  const baseImageStatus = useAppStore((s) => s.baseImageStatus);
  const refreshBaseImageStatus = useAppStore((s) => s.refreshBaseImageStatus);

  const [spawnOpen, setSpawnOpen] = useState(false);
  const [repoOpen, setRepoOpen] = useState(false);
  const [bakeOpen, setBakeOpen] = useState(false);

  useEffect(() => {
    init();
  }, [init]);

  const canSpawn = !!workspace && baseImageStatus === "ready";

  return (
    <div className="app">
      <WorkspaceBar onChooseRepo={() => setRepoOpen(true)} />
      {workspace && baseImageStatus === "missing" && (
        <MissingBaseImageBanner onBuild={() => setBakeOpen(true)} />
      )}
      <div className="body">
        <aside className="sidebar">
          <div className="sidebar-header">
            <span className="sidebar-title">Agents</span>
            <button
              className="primary"
              onClick={() => setSpawnOpen(true)}
              disabled={!canSpawn}
              title={
                !workspace
                  ? "Choose a repo first"
                  : baseImageStatus !== "ready"
                    ? `Base image '${workspace.base_image}' is not built yet`
                    : ""
              }
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
      {bakeOpen && workspace && (
        <BakeDialog
          imageName={workspace.base_image}
          onClose={() => setBakeOpen(false)}
          onSuccess={() => {
            setBakeOpen(false);
            void refreshBaseImageStatus();
          }}
        />
      )}
    </div>
  );
}
