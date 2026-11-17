import { AppShell } from "./components/AppShell";
import { BootScreen } from "./components/BootScreen";
import { useAppStore } from "./store";

/** The boot screen until the engine is ready, then the app. `AppShell` mounts
 *  only then, so its `init` (and every command the tree below it invokes) runs
 *  against an engine that can answer. */
export function App() {
  const bootStatus = useAppStore((s) => s.bootStatus);
  if (bootStatus.phase !== "ready") return <BootScreen status={bootStatus} />;
  return <AppShell />;
}
