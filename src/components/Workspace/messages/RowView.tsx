// Rendering the row model (rows.ts): a message or a run of steps. Shared by the
// desktop chat and the phone, which differ only in how they draw a message
// (`renderItem`).

import { useContext } from "react";
import { RowEnv } from "./host";
import { rowKey, type ViewItem } from "./pair";
import type { Row } from "./rows";
import { StepGroup } from "./StepGroup";

/** Stable React key for a row: an item by `rowKey` (its tool id, else its
 *  index), a run by its own key. */
export function rowViewKey(row: Row, items: ViewItem[]): string {
  if (row.kind === "steps") return row.run.key;
  return rowKey(items[row.index], row.index);
}

export function RowView({ row }: { row: Row }) {
  const env = useContext(RowEnv);
  if (!env) return null;
  const { agentId, provider, liveBusy, openTurnStart, renderItem } = env;
  if (row.kind === "item") {
    return <>{renderItem(row.index, liveBusy && row.index >= openTurnStart)}</>;
  }
  return (
    <StepGroup
      run={row.run}
      agentId={agentId}
      provider={provider}
      busy={liveBusy && row.run.start >= openTurnStart}
      tail={row.run.key === env.tailRunKey}
    />
  );
}
