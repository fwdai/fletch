import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { useAppStore } from "@/store";

// The last parent folder a project was created in, pre-filled next time.
const PARENT_KEY = "q2:newProjectParent";

/** Shared state for the New Project modal: destination parent (remembered),
 *  the shared GitHub connection state, and the directory picker. */
export function useNewProject() {
  const [parent, setParentState] = useState<string>(() => localStorage.getItem(PARENT_KEY) ?? "");
  // The store's connection state, refreshed here on open so a sign-in from
  // elsewhere (or since launch) is reflected; the ConnectGitHub flow updates
  // the same store field, so the views react without a manual re-probe.
  const gh = useAppStore((s) => s.github);
  const refreshGithub = useAppStore((s) => s.refreshGithub);

  useEffect(() => {
    void refreshGithub();
  }, [refreshGithub]);

  const setParent = useCallback((p: string) => {
    setParentState(p);
    localStorage.setItem(PARENT_KEY, p);
  }, []);

  const pickParent = useCallback(async () => {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Choose where to create the project",
      defaultPath: parent || undefined,
    });
    if (typeof picked === "string") setParent(picked);
  }, [parent, setParent]);

  return { parent, setParent, pickParent, gh };
}
