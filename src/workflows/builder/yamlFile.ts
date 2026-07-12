// File-system glue for YAML import/export (spec §14.3). The `wf_def_*_yaml`
// commands deal in YAML *text*, so the renderer owns picking the path and
// reading/writing the file — via the dialog + fs plugins, matching how the rest
// of the app touches the user's filesystem.

import { open, save } from "@tauri-apps/plugin-dialog";
import { readTextFile, writeTextFile } from "@tauri-apps/plugin-fs";
import { api } from "../../api";

const YAML_FILTERS = [{ name: "YAML", extensions: ["yaml", "yml"] }];

/** A filesystem-friendly stem for the default export filename. */
function slug(name: string): string {
  return (
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "workflow"
  );
}

/** Export a definition to a user-chosen `.yaml` file. Returns the written path,
 *  or `null` if the user dismissed the save dialog. */
export async function exportDefinitionYaml(id: string, name: string): Promise<string | null> {
  const yaml = await api.wfDefExportYaml(id);
  const path = await save({
    title: "Export workflow",
    defaultPath: `${slug(name)}.yaml`,
    filters: YAML_FILTERS,
  });
  if (!path) return null;
  await writeTextFile(path, yaml);
  return path;
}

/** Prompt for a `.yaml` file and return its contents, or `null` if the user
 *  dismissed the open dialog. */
export async function pickYamlText(): Promise<string | null> {
  const path = await open({
    title: "Import workflow",
    multiple: false,
    directory: false,
    filters: YAML_FILTERS,
  });
  if (typeof path !== "string") return null;
  return await readTextFile(path);
}
