import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { api, type GhStatus } from "../../api";

// The last parent folder a project was created in, pre-filled next time.
const PARENT_KEY = "q2:newProjectParent";

/** Shared state for the New Project modal: destination parent (remembered),
 *  the gh availability probe, and the directory picker. */
export function useNewProject() {
  const [parent, setParentState] = useState<string>(() => localStorage.getItem(PARENT_KEY) ?? "");
  const [gh, setGh] = useState<GhStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .ghStatus()
      .then((s) => {
        if (!cancelled) setGh(s);
      })
      .catch(() => {
        // If the probe itself fails, treat gh as unavailable so the gate shows
        // immediately rather than leaving the form live with a deferred error.
        if (!cancelled) {
          setGh({ installed: false, authenticated: false, login: null });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

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
