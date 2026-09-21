import { useCallback, useEffect, useState } from "react";
import { pickFolder } from "@/components/FolderPicker";
import { useAppStore } from "@/store";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";

// The last parent folder a project was created in, pre-filled next time.
const PARENT_KEY = "q2:newProjectParent";

/** Per environment, because a path on this Mac names nothing on a paired host.
 *  This Mac keeps the unsuffixed key it has always written. */
const parentKey = (envId: EnvironmentId) =>
  envId === LOCAL_ENVIRONMENT_ID ? PARENT_KEY : `${PARENT_KEY}:${envId}`;

/** Shared state for the New Project modal: destination parent (remembered),
 *  the shared GitHub connection state, and the directory picker. */
export function useNewProject() {
  // Read once per mount: the modal is mounted when it opens, and the user
  // cannot switch environments from inside it.
  const envId = useAppStore((s) => s.activeEnvironmentId);
  const [parent, setParentState] = useState<string>(
    () => localStorage.getItem(parentKey(envId)) ?? "",
  );
  // The store's connection state, refreshed here on open so a sign-in from
  // elsewhere (or since launch) is reflected; the ConnectGitHub flow updates
  // the same store field, so the views react without a manual re-probe.
  const gh = useAppStore((s) => s.github);
  const refreshGithub = useAppStore((s) => s.refreshGithub);

  useEffect(() => {
    void refreshGithub();
  }, [refreshGithub]);

  const setParent = useCallback(
    (p: string) => {
      setParentState(p);
      localStorage.setItem(parentKey(envId), p);
    },
    [envId],
  );

  const pickParent = useCallback(async () => {
    const picked = await pickFolder({
      title: "Choose where to create the project",
      // Nothing remembered: leave the start to the picker, which opens the
      // native dialog wherever it last was and the host browser at `~`.
      start: parent || undefined,
    });
    if (picked) setParent(picked);
  }, [parent, setParent]);

  return { parent, setParent, pickParent, gh };
}
