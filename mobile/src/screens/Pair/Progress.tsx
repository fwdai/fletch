import type { ReactNode } from "react";
import type { PairStep } from "../../remote";

/** What each step of a connection attempt means to someone holding the phone.
 *
 *  The relay line names the thing that actually takes the time: the LAN
 *  address is always dialled first and has to time out before the relay is
 *  tried, so a phone away from its Mac spends its first seconds on an address
 *  that cannot answer. Saying so is the difference between a wait and a hang.
 */
export const STEP_LABELS: Record<PairStep, string> = {
  connecting: "Starting…",
  lan: "Looking for your Mac on this network…",
  relay: "Not on this network — reaching your Mac through the relay…",
  registering: "Registering this device with your Mac…",
  greeting: "Saying hello…",
  workspace: "Loading your workspace…",
};

/** A line of text with the working dots in front of it: something is
 *  happening, and this is what. */
export function Working({ children }: { children: ReactNode }) {
  return (
    <div className="pair-step">
      <span className="working-dots">
        <i />
        <i />
        <i />
      </span>
      {children}
    </div>
  );
}

export function Progress({ step }: { step: PairStep }) {
  return <Working>{STEP_LABELS[step]}</Working>;
}
