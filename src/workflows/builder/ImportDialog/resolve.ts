// Pure resolution logic for the YAML import flow (spec §5.3, §14.3). The backend
// (`wf_def_import_yaml`) returns an `ImportReport`: the parsed spec with embedded
// agent specs (skills already pruned to what resolves locally) plus, per alias, a
// `local_match` if a custom agent of the same name exists. The UI lets the user
// pick, per alias, whether to map that alias onto their local custom agent or to
// keep the file's embedded spec; this module turns those choices into the final
// `Spec` handed to `wf_def_save`.

import type { AgentResolution, ImportReport, Spec } from "../../spec";

/** Per-alias import choice: adopt the local custom agent, or keep the embedded spec. */
export type AgentChoice = "map" | "embed";

/** The default choice for an alias: prefer the user's local agent when one exists
 *  (a name match is a strong signal it's the same role), else use the embedded spec. */
export function defaultChoice(r: AgentResolution): AgentChoice {
  return r.local_match ? "map" : "embed";
}

/** Build the initial choice map from the report's defaults. */
export function initialChoices(report: ImportReport): Record<string, AgentChoice> {
  const choices: Record<string, AgentChoice> = {};
  for (const r of report.agents) choices[r.alias] = defaultChoice(r);
  return choices;
}

/** Apply the user's per-alias choices to the imported spec, producing the spec to
 *  save. "map" attaches the local custom-agent id (which `resolveAgent` prefers over
 *  `base` at launch) while keeping the embedded fields as a portable fallback;
 *  "embed" (or a missing local match) keeps the embedded run-scoped spec untouched. */
export function applyResolutions(report: ImportReport, choices: Record<string, AgentChoice>): Spec {
  const agents = { ...report.spec.agents };
  for (const r of report.agents) {
    const choice = choices[r.alias] ?? defaultChoice(r);
    agents[r.alias] =
      choice === "map" && r.local_match
        ? { ...r.embedded, custom_agent: r.local_match.id }
        : { ...r.embedded };
  }
  return { ...report.spec, agents };
}
