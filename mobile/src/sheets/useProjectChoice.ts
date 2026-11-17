import { useEffect, useState } from "react";
import { useHost } from "../host/useHost";
import { entryOf, type Project, type ProjectEntry } from "../lib/projects";
import { type SheetName, useProjectEntries } from "../store";
import { copyProjectTo } from "../store/copyProject";
import { openSheetOn, sheetOf } from "../store/hostNav";

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
  /** The host a copy of the picked project is being made on for the sheet
   *  (`setHostKey`), while one is; else null. */
  preparing: string | null;
  /** Pick a project, and move the sheet to its speaking host. */
  pick(entry: ProjectEntry): void;
  /** Move the sheet to another host, keeping the project picked. A host
   *  without the project is given a copy first (`copyProjectTo`), and the
   *  sheet moves once that host lists it, if it is still up here on the same
   *  project. A pick while a copy is being made is ignored. Rejects, the sheet
   *  where it was, when the copy can't be made. */
  setHostKey(hostKey: string): Promise<void>;
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
  const [preparing, setPreparing] = useState<string | null>(null);
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
    preparing,
    pick: (e) => moveTo(e.hostKey, e),
    setHostKey: async (to) => {
      if (preparing || !entry || to === hostKey) return;
      if (entry.hosts.some((h) => h.hostKey === to)) return moveTo(to, entry);
      setPreparing(to);
      try {
        await copyProjectTo(to, entry);
      } finally {
        setPreparing(null);
      }
      // A sheet closed, moved or on another project since stays as the user
      // left it.
      if (sheetOf(hostKey, name)?.props.projectId === entry.project.id) moveTo(to, entry);
    },
  };
}
