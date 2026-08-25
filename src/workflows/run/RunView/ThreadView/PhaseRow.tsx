// ThreadView/PhaseRow — the named gap. Whenever no agent is streaming, this row
// says what the run is doing and how long it has been doing it. Quiet by design:
// a pulse, a label and a clock, not a banner (the banner is for states that need
// the human).

import { open as openExternal } from "@tauri-apps/plugin-shell";
import { type CSSProperties, useEffect, useState } from "react";
import { Icon, type IconName } from "../../../../components/Icon";
import { Loader } from "../../../../components/ui/Loader";
import { LiveTimer } from "../../../../components/Workspace/RunTimer";
import { GREEN } from "../../status";
import { isTerminalPhase, type Phase, type PhaseKind, phaseLabel } from "./phases";

/** Phases that are normally instant — the boundary commit usually lands in
 *  milliseconds. A row that flashes and vanishes is noise, so these only get
 *  announced once they have actually persisted. */
const TRANSIENT: ReadonlySet<PhaseKind> = new Set(["committing"]);
const TRANSIENT_DELAY_MS = 600;

export function PhaseRow({ phase, agentName }: { phase: Phase; agentName?: string }) {
  const persisted = usePersisted(
    `${phase.kind}:${phase.startedAt}`,
    TRANSIENT.has(phase.kind) ? TRANSIENT_DELAY_MS : 0,
  );
  if (isTerminalPhase(phase)) return <TerminalRow phase={phase} />;
  if (!persisted) return null;
  return (
    <div className="wf-phase" role="status" aria-live="polite">
      <Loader variant="accent" />
      <span className="wf-phase-label">{phaseLabel(phase, agentName)}</span>
      <span className="wf-phase-sep">·</span>
      <Icon name="clock" size={11} className="turn-clock-i" />
      <LiveTimer startedAt={phase.startedAt} />
    </div>
  );
}

/** Whether `delay` ms have passed since `key` last changed. `delay: 0` is
 *  immediate — the common case, so it never costs a render. */
function usePersisted(key: string, delay: number): boolean {
  const [persisted, setPersisted] = useState(delay === 0);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `key` is the change
  // signal rather than a value the body reads — a new phase restarts the wait.
  useEffect(() => {
    if (delay === 0) {
      setPersisted(true);
      return;
    }
    setPersisted(false);
    const t = window.setTimeout(() => setPersisted(true), delay);
    return () => window.clearTimeout(t);
  }, [key, delay]);
  return persisted;
}

/** The run's last word: what happened, and the one link that follows from it. */
function TerminalRow({ phase }: { phase: Phase }) {
  const url = phase.url;
  const tone =
    phase.kind === "done" ? GREEN : phase.kind === "failed" ? "var(--danger)" : "var(--fg-3)";
  const icon: IconName =
    phase.kind === "done" ? "check" : phase.kind === "failed" ? "close" : "stop";
  return (
    <div className={`wf-phase term ${phase.kind}`} style={{ "--term-tone": tone } as CSSProperties}>
      <span className="wf-phase-mark" aria-hidden="true">
        <Icon name={icon} size={12} />
      </span>
      <div className="wf-phase-term-body">
        <div className="wf-phase-term-title">{phaseLabel(phase)}</div>
        {phase.kind !== "done" && phase.detail && (
          <div className="wf-phase-term-detail">{phase.detail}</div>
        )}
        {phase.kind === "done" && phase.detail && (
          <div className="wf-phase-term-detail mono">{phase.detail}</div>
        )}
        {url && (
          <button
            type="button"
            className="btn-t outline wf-phase-pr"
            onClick={() => {
              void openExternal(url).catch(() => {});
            }}
          >
            <Icon name="pr" size={12} /> View pull request
          </button>
        )}
      </div>
    </div>
  );
}
