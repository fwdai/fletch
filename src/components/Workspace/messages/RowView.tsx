// Rendering the row model (rows.ts): a message, a run of steps, or a settled
// turn's folded work. Shared by the transcript's top level and the inside of an
// opened fold, which hold the same kinds of rows — and by the desktop chat and
// the phone, which differ only in how they draw a message (`renderItem`).

import { createContext, memo, type ReactNode, useContext, useMemo } from "react";
import { fmtDur } from "@/util/format";
import { DeepOpen, DisclosureBody, useDisclosure, useFocusInside, useRevealed } from "./Disclosure";
import { useTranscriptHost } from "./host";
import { rowKey, type ViewItem } from "./pair";
import type { Fold, Row } from "./rows";
import { StepGlyph, StepGroup, SummaryLine } from "./StepGroup";
import { summarizeSteps, summaryParts } from "./steps";

/** What every row needs from the transcript it sits in. Read through context
 *  so a closed fold doesn't re-render as the live turn streams below it; only
 *  the rows actually on the page do. */
export interface RowEnvValue {
  agentId: string;
  provider: string | undefined;
  items: ViewItem[];
  liveBusy: boolean;
  openTurnStart: number;
  /** Key of the run the transcript ends on, if it ends on one. */
  tailRunKey: string | null;
  /** The app's own rendering of one item that isn't a step — a prompt, the
   *  agent's prose, a sub-agent card, a question for the user. `busy` is
   *  whether the item's turn is live. */
  renderItem: (index: number, busy: boolean) => ReactNode;
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
  const { agentId, provider, liveBusy, openTurnStart, renderItem } = env;
  switch (row.kind) {
    case "item":
      return <>{renderItem(row.index, liveBusy && row.index >= openTurnStart)}</>;
    case "steps":
      return (
        <StepGroup
          run={row.run}
          agentId={agentId}
          provider={provider}
          busy={liveBusy && row.run.start >= openTurnStart}
          tail={row.run.key === env.tailRunKey}
          renderItem={renderItem}
        />
      );
    case "fold":
      return <WorkFold fold={row.fold} agentId={agentId} />;
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
  const { taskFor } = useTranscriptHost();
  const parts = useMemo(
    () => summaryParts(summarizeSteps(fold.items, taskFor)),
    [fold.items, taskFor],
  );
  const lead = fold.runSec !== undefined ? `Worked for ${fmtDur(fold.runSec)}` : undefined;

  return (
    <div className={shown ? "turnfold is-open" : "turnfold"} data-turnfold={fold.key}>
      <button
        type="button"
        className="steprun-head turnfold-head"
        aria-expanded={shown}
        onClick={toggle}
      >
        <StepGlyph icon="clock" />
        <span className="steprun-text">
          <SummaryLine lead={lead} parts={parts} />
        </span>
      </button>
      <DisclosureBody
        open={shown}
        instant={instant || (revealed && !open)}
        className="turnfold-body"
      >
        <DeepOpen.Provider value={deep}>
          <div className="turnfold-list">
            <FoldRows rows={fold.rows} />
          </div>
        </DeepOpen.Provider>
      </DisclosureBody>
    </div>
  );
});
