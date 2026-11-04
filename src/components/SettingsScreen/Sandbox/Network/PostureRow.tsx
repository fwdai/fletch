import { EGRESS_MODES, type EgressMode, parseEgressMode } from "@/api";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { EGRESS_MODE_HINTS, EGRESS_MODE_LABELS } from "@/util/egressMode";
import { SetGroup, SetRow, SetSeg } from "../../primitives";

/** The one control: what a VM does when an agent reaches a host Fletch does
 *  not allow. Open by default; a project can pick its own on its page. On a
 *  host from before postures the row says so and nothing is offered: that host
 *  refuses unlisted hosts, as it always did. */
export function PostureRow() {
  const gate = useGate("egressPrompt");
  const raw = useAppStore((s) => s.boxEgressMode);
  const save = useAppStore((s) => s.saveBoxSetting);
  const mode = parseEgressMode(raw);

  const pick = (next: EgressMode) => {
    // Blank restores the default (open), as every other box knob does.
    void save("box_egress_mode", next === "open" ? "" : next);
  };

  return (
    <SetGroup>
      <SetRow
        title="New hosts"
        sub={
          gate ??
          `What a VM does when an agent reaches a host Fletch doesn't allow. ${EGRESS_MODE_HINTS[mode]}`
        }
      >
        {!gate && (
          <SetSeg
            value={mode}
            options={EGRESS_MODES.map((m) => ({
              value: m,
              label: EGRESS_MODE_LABELS[m],
              tip: EGRESS_MODE_HINTS[m],
            }))}
            onChange={pick}
          />
        )}
      </SetRow>
    </SetGroup>
  );
}
