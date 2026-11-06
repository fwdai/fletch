import { EGRESS_MODES, type EgressMode, parseEgressMode } from "@/api";
import { Select } from "@/components/ui/Select";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import {
  EGRESS_MODE_HINTS,
  EGRESS_MODE_KEY,
  EGRESS_MODE_LABELS,
  effectiveEgressMode,
} from "@/util/egressMode";
import { useProjectSettings } from "@/util/useProjectSettings";
import { SetGroup, SetRow, SetSeg } from "../../primitives";

/** The one control: what a VM does when an agent reaches a host Fletch does
 *  not allow. Open by default. With `projectId`, that project's own choice
 *  over the app's, which its first option names. On a host from before
 *  postures the row says so and nothing is offered: that host refuses
 *  unlisted hosts, as it always did. */
export function PostureRow({ projectId }: { projectId?: string }) {
  const gate = useGate("egressPrompt");
  const settingsGate = useGate("projectSettings");
  const raw = useAppStore((s) => s.boxEgressMode);
  const saveGlobal = useAppStore((s) => s.saveBoxSetting);
  const { settings, save } = useProjectSettings(projectId);
  const { mode, overridden } = effectiveEgressMode(settings?.[EGRESS_MODE_KEY], raw);
  const options = EGRESS_MODES.map((m) => ({ value: m, label: EGRESS_MODE_LABELS[m] }));

  const pick = (next: EgressMode) => {
    // Blank restores the default (open), as every other box knob does.
    void saveGlobal("box_egress_mode", next === "open" ? "" : next);
  };

  return (
    <SetGroup>
      <SetRow
        title="New hosts"
        sub={
          gate ??
          `What a VM does when an agent reaches a host Fletch doesn't allow. ${EGRESS_MODE_HINTS[mode]}`
        }
        inherited={
          projectId ? { overridden, onRevert: () => save(EGRESS_MODE_KEY, null) } : undefined
        }
      >
        {!gate &&
          (projectId ? (
            <Select<string>
              value={overridden ? mode : "default"}
              ariaLabel="New hosts"
              disabled={!!settingsGate || settings === null}
              options={[
                {
                  value: "default",
                  label: `App default · ${EGRESS_MODE_LABELS[parseEgressMode(raw)]}`,
                },
                ...options,
              ]}
              onChange={(next) => save(EGRESS_MODE_KEY, next === "default" ? null : next)}
            />
          ) : (
            <SetSeg
              value={mode}
              options={options.map((o) => ({ ...o, tip: EGRESS_MODE_HINTS[o.value] }))}
              onChange={pick}
            />
          ))}
      </SetRow>
    </SetGroup>
  );
}
