import { useEffect } from "react";
import { Sidebar } from "./components/Sidebar";
import { AgentPanes } from "./components/AgentPanes";
import { useAppStore } from "./store";

export function App() {
  const init = useAppStore((s) => s.init);

  useEffect(() => {
    init();
  }, [init]);

  return (
    <div className="app">
      <header className="bar">
        <span className="logo">Quorum</span>
      </header>
      <div className="body">
        <Sidebar />
        <main className="main">
          <AgentPanes />
        </main>
      </div>
    </div>
  );
}
