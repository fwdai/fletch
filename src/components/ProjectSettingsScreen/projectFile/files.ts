import type { ProjectFileReport, ProjectFileWhich } from "@/api";

export const FILE_LABEL: Record<ProjectFileWhich, string> = {
  shared: ".fletch/settings.json",
  local: ".fletch/settings.local.json",
};

export const fileReport = (files: ProjectFileReport[], which: ProjectFileWhich) =>
  files.find((f) => f.path === FILE_LABEL[which]);

/** The first file, the team's before yours, whose report `holds`. */
export const fileHolding = (
  files: ProjectFileReport[],
  holds: (file: ProjectFileReport) => boolean,
): ProjectFileWhich | null =>
  (["shared", "local"] as const).find((which) => {
    const file = fileReport(files, which);
    return !!file && holds(file);
  }) ?? null;
