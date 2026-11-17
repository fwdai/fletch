import { useEffect } from "react";
import { useHost } from "../host/useHost";
import { entryOf, type Project, type ProjectEntry } from "../lib/projects";
import { type SheetName, useProjectEntries } from "../store";
import { openSheetOn } from "../store/hostNav";

/** What a sheet that starts something in a project is acting on. */
export interface ProjectChoice {
  /** Every paired host's projects, to pick from. */
  entries: ProjectEntry[];
  /** The project picked, undefined only when no host has one. */
  entry: ProjectEntry | undefined;
  /** The host the sheet acts on: the one its props name. */
  hostKey: string;
  /** The picked project as that host has it: its repos are that host's. */
  project: Project | undefined;
  /** Pick a project, and move the sheet to its speaking host. */
  pick(entry: ProjectEntry): void;
  /** Move the sheet to another host, keeping the project picked. */
  setHostKey(hostKey: string): void;
}

/** The project the `name` sheet starts something in, and the host it acts on.
 *  Both are the sheet's props, so the app renders it on that host and the
 *  host's own actions find it there: the project its `projectId` names on its
 *  host, else the first listed. Opened with none named, it is moved to that
 *  first project's speaking host at once. */
export function useProjectChoice(
  name: SheetName,
  open: boolean,
  projectId: string | undefined,
): ProjectChoice {
  const entries = useProjectEntries();
  const { hostKey } = useHost();
  const entry = entryOf(entries, hostKey, projectId) ?? entries[0];
  const moveTo = (to: string, e: ProjectEntry | undefined) =>
    openSheetOn(to, name, e ? { projectId: e.project.id } : {});

  const first = entries[0];
  useEffect(() => {
    if (open && projectId === undefined && first) {
      openSheetOn(first.hostKey, name, { projectId: first.project.id });
    }
  }, [open, projectId, first, name]);

  return {
    entries,
    entry,
    hostKey,
    project: entry?.hosts.find((h) => h.hostKey === hostKey)?.project,
    pick: (e) => moveTo(e.hostKey, e),
    setHostKey: (to) => moveTo(to, entry),
  };
}
