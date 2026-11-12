import type { ProjectFileEdit, ProjectFileReport, ProjectFileWhich } from "@/api";
import { fileReport } from "../projectFile/files";

/** The file whose `seed.enabled` decides: the local file's word stands over
 *  the team's. Null when neither says, which leaves the starting disk on. */
export const enabledSource = (files: ProjectFileReport[]): ProjectFileWhich | null =>
  (["local", "shared"] as const).find((which) => fileReport(files, which)?.seed.enabled != null) ??
  null;

/** The edit that turns the starting disk `on` or off. It goes to the local
 *  file once that file says, since nothing else would change the outcome,
 *  else to `target`; and it takes the setting out of the file when what lies
 *  beneath (the team's file, then the default) already says `on`. */
export function enabledEdit(
  files: ProjectFileReport[],
  target: ProjectFileWhich,
  on: boolean,
): { file: ProjectFileWhich; change: ProjectFileEdit } {
  const file = enabledSource(files) === "local" ? "local" : target;
  const beneath = file === "local" ? (fileReport(files, "shared")?.seed.enabled ?? true) : true;
  return { file, change: { seedEnabled: on === beneath ? null : on } };
}
