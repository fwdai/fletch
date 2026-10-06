import { useState } from "react";
import type { ContextAssertion, ContextGraph } from "@/api";
import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { authorLabel, contradictedBy, historyOf, sourceLabel, statusVariant } from "./format";
import { RetractDialog } from "./RetractDialog";
import { SupersedeDialog } from "./SupersedeDialog";

/** One current assertion: statement, rationale, who said it and where it came
 *  from, and the three things the user can do with it — change it (a new
 *  assertion superseding this one), retract it, or read how it got here. */
export function AssertionRow({
  assertion,
  graph,
  projectId,
}: {
  assertion: ContextAssertion;
  graph: ContextGraph;
  projectId: string;
}) {
  const [dialog, setDialog] = useState<"supersede" | "retract" | null>(null);
  const [showHistory, setShowHistory] = useState(false);
  const contradicted = contradictedBy(graph, assertion);
  const history = showHistory ? historyOf(graph, assertion.id) : [];
  const settled = assertion.status === "retracted" || assertion.status === "abandoned";

  return (
    <div className={`pc-assertion ${assertion.stance}`}>
      <div className="pc-statement text-sm">{assertion.statement}</div>
      {assertion.rationale && <div className="pc-meta text-xs">{assertion.rationale}</div>}
      <div className="pc-badges text-xs">
        <Badge variant={statusVariant(assertion.status)}>{assertion.status}</Badge>
        {assertion.stance === "rejected" && <Badge>rejected</Badge>}
        <Badge hint={sourceLabel(assertion.source)}>
          {authorLabel(assertion.author)} · {sourceLabel(assertion.source)}
        </Badge>
        {contradicted.length > 0 && (
          <Badge variant="warn" hint={contradicted.map((c) => c.statement).join("\n")}>
            contradicted
          </Badge>
        )}
        <span className="pc-actions">
          {!settled && (
            <Button variant="link" size="sm" onClick={() => setDialog("supersede")}>
              Change
            </Button>
          )}
          {!settled && (
            <Button variant="link" size="sm" danger onClick={() => setDialog("retract")}>
              Retract
            </Button>
          )}
          {assertion.supersedes && (
            <Button variant="link" size="sm" onClick={() => setShowHistory((v) => !v)}>
              {showHistory ? "Hide history" : "History"}
            </Button>
          )}
        </span>
      </div>

      {showHistory && (
        <div className="pc-history text-xs">
          <span className="pc-meta">Replaced: {assertion.supersedes?.reasoning}</span>
          {history.map((prev, i) => {
            // The reasoning for replacing `prev` sits on its successor.
            const successor = i === 0 ? assertion : history[i - 1];
            return (
              <div key={prev.id}>
                <div>{prev.statement}</div>
                <div className="pc-meta">
                  {prev.stance} · {authorLabel(prev.author)}
                  {prev.supersedes && successor !== assertion && (
                    <> · replaced: {prev.supersedes.reasoning}</>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {dialog === "supersede" && (
        <SupersedeDialog
          assertion={assertion}
          projectId={projectId}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog === "retract" && (
        <RetractDialog
          assertion={assertion}
          projectId={projectId}
          onClose={() => setDialog(null)}
        />
      )}
    </div>
  );
}
