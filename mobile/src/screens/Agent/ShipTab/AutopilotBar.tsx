import { useMemo, useState } from "react";
import { Toggle } from "../../../components/ui";
import { ignore } from "../../../lib/ignore";
import { useStore } from "../../../store";
import { agentCheckouts } from "../../../store/autopilot";
import { describeAutopilot } from "./autopilot";

/** Under the strip: what the host's autopilot is doing for this agent, and the
 *  switch that pauses it. Renders nothing on a host without autopilot. */
export function AutopilotBar({ agentId }: { agentId: string }) {
  const autopilot = useStore((s) => s.autopilot);
  const canSet = useStore((s) => s.hostSupports("autopilot_set"));
  const setAgentAutopilot = useStore((s) => s.setAgentAutopilot);
  // One flip at a time: the switch settles on the host's answer, not the tap.
  const [pending, setPending] = useState(false);
  const view = useMemo(
    () => describeAutopilot(agentCheckouts(autopilot, agentId), canSet),
    [autopilot, agentId, canSet],
  );
  if (!view) return null;

  const flip = (enabled: boolean) => {
    setPending(true);
    // A failure lands in `lastError`; the switch stays where the host has it.
    void setAgentAutopilot(agentId, enabled)
      .catch(ignore)
      .finally(() => setPending(false));
  };

  return (
    <div className={`ship-auto${view.busy ? " busy" : ""}`}>
      <span className="txt">{view.text}</span>
      {view.switchable && (
        <Toggle on={view.on} onChange={flip} disabled={pending} label="Autopilot" />
      )}
    </div>
  );
}
