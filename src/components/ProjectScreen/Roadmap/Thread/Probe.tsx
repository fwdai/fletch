import type { ReactNode } from "react";
import { ToolRow } from "@/components/Workspace/messages/ToolRow";
import { FINDING_TAG, type Finding } from "../types";

/** Render `backticked` spans in a finding as inline code. */
function ticks(text: string): ReactNode[] {
  // Odd indices are the captured groups, i.e. what was inside the backticks.
  return text.split(/`([^`]+)`/g).map((part, i) =>
    i % 2 === 1 ? (
      // biome-ignore lint/suspicious/noArrayIndexKey: a split of fixed text — the pieces never reorder
      <code key={i}>{part}</code>
    ) : (
      part
    ),
  );
}

/** The repo check the PM runs before proposing anything — collapsed to its
 *  one-line summary, expanding to the findings. Uses the same row chrome as a
 *  tool call in the agent transcript, because that is what it is. */
export function Probe({ summary, findings }: { summary: string; findings: Finding[] }) {
  return (
    <ToolRow
      name="Repo check"
      icon="cube"
      summary={summary}
      expanded={
        <ul className="rm-findings">
          {findings.map((f) => (
            <li key={f.text} className={`f-${f.kind}`}>
              <span className="rm-f-tag mono text-xs">{FINDING_TAG[f.kind]}</span>
              <span className="rm-f-t text-sm">{ticks(f.text)}</span>
            </li>
          ))}
        </ul>
      }
    />
  );
}
