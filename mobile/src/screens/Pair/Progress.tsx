import type { ReactNode } from "react";
import type { PairStep } from "../../remote";

/** What each step of a connection attempt means to someone holding the phone.
 *
 *  The second line names the thing that actually takes the time: the LAN
 *  address is always dialled first and has to time out before the relay is
 *  tried, so a phone away from its Mac spends its first seconds on an address
 *  that cannot answer. Saying so is the difference between a wait and a hang —
 *  in the user's terms (a network, the internet), not the relay's.
 */
export const STEP_LABELS: Record<PairStep, string> = {
  connecting: "Starting…",
  lan: "Looking for your Mac on this network…",
  relay: "Not on the same network — connecting over the internet…",
  registering: "Registering this device with your Mac…",
  requesting: "Asking your Mac…",
  confirming: "Accept on your Mac — check it shows the same code.",
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
