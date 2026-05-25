import type { AgentView } from "../api";
import { useAppStore } from "../store";

interface Props {
  agentId: string;
  current: AgentView;
  /** When true (e.g. mid-turn in custom view), disable the toggle so
   *  switching doesn't truncate an in-flight response. */
  disabled?: boolean;
}

/** Segmented Custom / Native toggle shared by both agent views. The
 *  click triggers backend `switch_view`, which tears down the current
 *  process and resumes the same session in the other shape. */
export function ViewToggle({ agentId, current, disabled }: Props) {
  const switchView = useAppStore((s) => s.switchView);
  const switching = useAppStore((s) => s.switchInFlight[agentId] ?? false);
  const lockedOut = disabled || switching;

  function flip(to: AgentView) {
    if (lockedOut || to === current) return;
    void switchView(agentId, to);
  }

  return (
    <div
      className="viewtoggle"
      role="tablist"
      aria-label="Agent view"
      data-disabled={lockedOut || undefined}
    >
      <button
        type="button"
        role="tab"
        aria-selected={current === "custom"}
        className={current === "custom" ? "active" : ""}
        onClick={() => flip("custom")}
        disabled={lockedOut}
        title={lockedOut ? "Wait for the current turn to finish" : "Custom UI"}
      >
        Custom
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={current === "native"}
        className={current === "native" ? "active" : ""}
        onClick={() => flip("native")}
        disabled={lockedOut}
        title={lockedOut ? "Wait for the current turn to finish" : "Native UI"}
      >
        Native
      </button>
    </div>
  );
}
