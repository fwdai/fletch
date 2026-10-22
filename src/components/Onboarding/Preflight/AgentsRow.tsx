// Row 01 · Agent CLIs. The one real requirement: without an agent nothing
// runs, so it is checked first. Found agents are named; none found pauses the
// sequence and points at the install panel on the right, with a quiet way
// past for someone who will install in their own terminal.

import type { OnboardingSetup } from "../useSetup";
import { CheckRow } from "./CheckRow";
import type { CheckState } from "./usePreflight";

export function AgentsRow({
  setup,
  state,
  active,
  onInspect,
  onSkip,
}: {
  setup: OnboardingSetup;
  state: CheckState;
  active: boolean;
  onInspect: () => void;
  onSkip: () => void;
}) {
  const { agents, providerPaths, detected } = setup;
  const names = agents.filter((p) => !!providerPaths[p.id]).map((p) => p.label);

  let detail: string;
  switch (state) {
    case "pending":
      detail = "Claude Code, Codex, Cursor and more";
      break;
    case "checking":
      detail = "Scanning this machine for agent CLIs…";
      break;
    case "ok":
      detail = `${detected} found · ${names.join(", ")}`;
      break;
    case "skipped":
      detail = "Skipped · install one anytime in Settings › Providers";
      break;
    default:
      detail = "None installed yet";
  }

  return (
    <CheckRow
      index={0}
      title="Agent CLIs"
      detail={detail}
      state={state}
      active={active}
      onInspect={onInspect}
    >
      {state === "attention" && (
        <>
          <p className="pf-note">
            Fletch directs the agent CLIs you already pay for. Install one from the panel on the
            right — it's a single click, and this row turns green on its own.
          </p>
          <button type="button" className="ob-skiplink" onClick={onSkip}>
            I'll install one later
          </button>
        </>
      )}
    </CheckRow>
  );
}
