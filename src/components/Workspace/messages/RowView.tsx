// Rendering the row model (rows.ts): a message, a run of steps, or a settled
// turn's folded work. Shared by the transcript's top level and the inside of an
// opened fold, which hold the same kinds of rows.

import { createContext, memo, useContext, useMemo } from "react";
import type { AgentRecord } from "@/api";
import { fmtDur } from "../RunTimer";
import { DeepOpen, DisclosureBody, useDisclosure, useFocusInside, useRevealed } from "./Disclosure";
import { MessageItem } from "./MessageItem";
import { rowKey, type ViewItem } from "./pair";
import type { Fold, Row } from "./rows";
import { StepGlyph, StepGroup, SummaryLine } from "./StepGroup";
import { summarizeSteps, summaryParts } from "./steps";

/** What every row needs from the transcript it sits in. Read through context
 *  so a closed fold doesn't re-render as the live turn streams below it; only
 *  the rows actually on the page do. */
export interface RowEnvValue {
  agent: AgentRecord;
  items: ViewItem[];
  turnIds: (number | undefined)[];
  liveBusy: boolean;
  openTurnStart: number;
  /** Key of the run the transcript ends on, if it ends on one. */
  tailRunKey: string | null;
}

export const RowEnv = createContext<RowEnvValue | null>(null);

/** Stable React key for a row: an item by `rowKey` (its tool id, else its
 *  index), a run or fold by its own key. */
export function rowViewKey(row: Row, items: ViewItem[]): string {
  if (row.kind === "steps") return row.run.key;
  if (row.kind === "fold") return row.fold.key;
  return rowKey(items[row.index], row.index);
}

export function RowView({ row }: { row: Row }) {
  const env = useContext(RowEnv);
  if (!env) return null;
  const { agent, items, turnIds, liveBusy, openTurnStart } = env;
  switch (row.kind) {
    case "item":
      return (
        <MessageItem
          item={items[row.index]}
          provider={agent.provider}
          agentId={agent.id}
          busy={liveBusy && row.index >= openTurnStart}
          turnId={turnIds[row.index]}
        />
      );
    case "steps":
      return (
        <StepGroup
          run={row.run}
          agentId={agent.id}
          provider={agent.provider}
          busy={liveBusy && row.run.start >= openTurnStart}
          tail={row.run.key === env.tailRunKey}
        />
      );
    case "fold":
      return <WorkFold fold={row.fold} agentId={agent.id} />;
  }
}

/** An opened fold's rows. Its own component so only an open fold reads the
 *  transcript's context. */
function FoldRows({ rows }: { rows: Row[] }) {
  const env = useContext(RowEnv);
  if (!env) return null;
  return (
    <>
      {rows.map((row) => (
        <RowView key={rowViewKey(row, env.items)} row={row} />
      ))}
    </>
  );
}

/** A settled turn's work behind one line: how long it ran and what it did.
 *  Opens to the turn as it happened — what the agent said along the way, its
 *  runs of steps, the sub-agents it started. */
const WorkFold = memo(function WorkFold({ fold, agentId }: { fold: Fold; agentId: string }) {
  const { open, deep, instant, reveal, toggle } = useDisclosure();
  const revealed = useRevealed(fold.items);
  useFocusInside(agentId, fold.items, reveal);
  const shown = open || revealed;
  const parts = useMemo(() => summaryParts(summarizeSteps(fold.items)), [fold.items]);
  const lead = fold.runSec !== undefined ? `Worked for ${fmtDur(fold.runSec)}` : undefined;

  return (
    <div className={shown ? "fold is-open" : "fold"} data-fold={fold.key}>
      <button type="button" className="steps-head fold-head" aria-expanded={shown} onClick={toggle}>
        <StepGlyph icon="clock" />
        <span className="steps-text">
          <SummaryLine lead={lead} parts={parts} />
        </span>
      </button>
      <DisclosureBody open={shown} instant={instant || (revealed && !open)} className="fold-body">
        <DeepOpen.Provider value={deep}>
          <div className="fold-list">
            <FoldRows rows={fold.rows} />
          </div>
        </DeepOpen.Provider>
      </DisclosureBody>
    </div>
  );
});
