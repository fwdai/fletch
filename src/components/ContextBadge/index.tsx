import { useCallback, useRef, useState } from "react";
import { hasUsage, priceSnapshot, reportsUsage, unspentSnapshot } from "@/adapters/usage";
import type { AgentRecord } from "@/api";
import { useAppStore } from "@/store";
import { compactBlocker, isCompacting, supportsCompaction } from "@/store/compaction";
import { useDismiss } from "@/util/hooks";
import { useShortcutKeys } from "@/util/shortcuts";
import { Breakdown } from "./Breakdown";
import { Chip } from "./Chip";
import { CompactButton } from "./CompactButton";
import { CompactFooter } from "./CompactFooter";
import { type CompactPhase, contextView } from "./model";

/** The conversation header's context badge (see the Context Badge component
 *  spec): how much of the model's window the conversation fills, a breakdown
 *  that opens on hover and pins on click, and — for an agent that can — the
 *  one action that matters when context runs low, compact.
 *
 *  There from the start, for every agent that reports usage (antigravity's does
 *  not): until a call has measured the window it reads as unmeasured rather
 *  than being absent, so a first turn that runs for half an hour shows where
 *  its numbers will be. Compacting waits for spend — there is nothing to
 *  summarize before it. */
export function ContextBadge({ agent }: { agent: AgentRecord }) {
  const show = useAppStore((s) => s.features.tokenUsage);
  const stored = useAppStore((s) => s.usage[agent.id]);
  const catalog = useAppStore((s) => s.modelCatalog);
  const compacting = useAppStore((s) => isCompacting(s, agent.id));
  const blocker = useAppStore((s) => compactBlocker(s, agent.id));
  const compactAgent = useAppStore((s) => s.compactAgent);
  const keys = useShortcutKeys("compactAgent");

  const ref = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState(false);
  const [pinned, setPinned] = useState(false);
  const unpin = useCallback(() => setPinned(false), []);
  useDismiss(ref, pinned, unpin);

  if (!show || !reportsUsage(agent.provider)) return null;

  const usage = stored ?? unspentSnapshot(agent.model);
  const phase: CompactPhase = compacting ? "compacting" : usage.context.compacted ? "done" : "idle";
  const view = contextView(usage, catalog, phase);
  const open = hover || pinned;
  const canCompact = hasUsage(usage) && supportsCompaction(agent.provider);
  // Once a compaction has landed there is nothing left to compact until the
  // conversation moves on, so the segment steps aside for the receipt.
  const showSegment = canCompact && phase !== "done";
  const compact = () => void compactAgent(agent.id);

  return (
    <div
      ref={ref}
      className={`husage tone-${view.tone}${phase === "compacting" ? " compacting" : ""}${open ? " open" : ""}`}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <div className={`husage-group${showSegment ? " has-act" : ""}`}>
        <Chip view={view} phase={phase} open={open} onClick={() => setPinned((p) => !p)} />
        {showSegment && <CompactButton blocker={blocker} keys={keys} onCompact={compact} />}
      </div>
      {open && (
        <Breakdown
          usage={usage}
          view={view}
          cost={priceSnapshot(catalog, usage)}
          footer={
            canCompact ? (
              <CompactFooter
                phase={phase}
                freed={usage.context.compacted?.freed ?? null}
                blocker={blocker}
                keys={keys}
                onCompact={compact}
              />
            ) : undefined
          }
        />
      )}
    </div>
  );
}
