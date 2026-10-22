import type { AgentRecord } from "@desktop/api/types/agent";
import type { FooterAction } from "@desktop/components/RightPanel/GitPanel/footerModel";
import { useEffect, useState } from "react";
import { baseOf, isAgentBusy } from "../../../lib/agents";
import { ignore } from "../../../lib/ignore";
import { openExternal } from "../../../lib/links";
import { useStore } from "../../../store";
import { type ShipFacts, shipOp } from "./actions";

/** How long an armed merge waits for its second tap. */
export const ARM_MS = 4000;

/** The one dispatch for a footer key, shared by the footer and the More sheet
 *  so a tap means the same thing from either. What a key does is `shipOp`'s
 *  call; this runs it. A merge cannot be undone, so it arms on the first tap
 *  and merges on the second; the arming holds the agent it was armed for and
 *  lapses on its own. */
export function useShipActions(agent: AgentRecord, onDelegated?: () => void) {
  const pr = useStore((s) => s.prStates[agent.id] ?? null);
  const checks = useStore((s) => s.prChecks[agent.id] ?? null);
  const comments = useStore((s) => s.prComments[agent.id] ?? null);
  const openSheet = useStore((s) => s.openSheet);
  const delegateGit = useStore((s) => s.delegateGit);
  const mergePr = useStore((s) => s.mergePr);
  const archive = useStore((s) => s.archive);
  // A trigger sent mid-turn folds into the running turn instead of running as
  // its own (the desktop queues it until idle); v1 on the phone simply waits.
  const busy = useStore((s) => isAgentBusy(s, agent));
  const [armed, setArmed] = useState<string | null>(null);
  const [merging, setMerging] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const id = setTimeout(() => setArmed(null), ARM_MS);
    return () => clearTimeout(id);
  }, [armed]);
  const isArmed = armed === agent.id;
  const facts: ShipFacts = { base: baseOf(agent), pr, checks, comments };
  const opOf = (key: string) => shipOp(key, facts);

  /** Resolves to whether the action is finished with (false for a merge that
   *  only armed), so a sheet knows whether to close. */
  const run = async (key: string): Promise<boolean> => {
    const op = opOf(key);
    switch (op?.kind) {
      case "delegate":
        await delegateGit(agent.id, op.action, op.params).catch(ignore);
        onDelegated?.();
        return true;
      case "merge":
        if (!isArmed) {
          setArmed(agent.id);
          return false;
        }
        setArmed(null);
        setMerging(true);
        try {
          await mergePr(agent.id);
        } catch {
          // The store's guard has already put the message in `lastError`.
        } finally {
          setMerging(false);
        }
        return true;
      case "archive":
        await archive(agent.id).catch(ignore);
        return true;
      case "github":
        openExternal(op.url);
        return true;
      case "manual":
        openSheet("pr", { agentId: agent.id });
        return true;
      default:
        // `shipView` never offers a key the phone can't run.
        return true;
    }
  };

  const label = (a: FooterAction) =>
    a.key !== "merge"
      ? a.label
      : merging
        ? "Merging…"
        : isArmed
          ? `Tap again to merge #${pr?.number}`
          : a.label;
  const isDelegate = (a: FooterAction) => opOf(a.key)?.kind === "delegate";
  const disabled = (a: FooterAction) =>
    !!a.disabled || (a.key === "merge" ? merging : isDelegate(a) && busy);

  return { run, label, disabled, isDelegate, busy };
}
