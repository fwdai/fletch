import type { RefObject } from "react";
import type { AgentRecord } from "@/api";
import { AgentIdentityChip, useAgentIdentityLabel } from "@/components/AgentIdentityChip";
import { IsolationDetail } from "@/components/TitleBar/WorkspaceStatus/IsolationDetail";
import { AnchoredPopover } from "@/components/ui";
import { providerLabel } from "@/data/providers";

/** Floats off the agent row's sandbox glyph (`anchorRef`) on hover: where the
 *  workspace runs, then which agent runs in it. The two read as one fact about
 *  the workspace, so the agent's line borrows the sandbox's layout. Pure
 *  display, no actions. */
export function RuntimePopover({
  agent,
  anchorRef,
}: {
  agent: AgentRecord;
  anchorRef: RefObject<HTMLElement | null>;
}) {
  const provider = providerLabel(agent.provider);
  // The custom agent's name, the model and the effort; just the provider's
  // name again until a model is known.
  const identity = useAgentIdentityLabel(agent);

  return (
    <AnchoredPopover
      anchorRef={anchorRef}
      className="ag-stats-pop ag-runtime-pop"
      // Body-level in the DOM, but still the row's child to React: a click
      // here would otherwise select the row.
      onClick={(e) => e.stopPropagation()}
    >
      <IsolationDetail agent={agent} />
      <div className="ws-pop-div" />
      <div className="ws-pop-iso">
        <AgentIdentityChip agent={agent} size={14} tip={false} />
        <div className="ws-pop-iso-text">
          <div className="ws-pop-iso-title">{provider}</div>
          {identity !== provider && <div className="ws-pop-iso-hint">{identity}</div>}
        </div>
      </div>
    </AnchoredPopover>
  );
}
