import { useState } from "react";
import type { ContextAssertion, ContextGraph } from "@/api";
import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import {
  authorLabel,
  historyOf,
  openTensions,
  provenanceLabel,
  resolutionLabel,
  resolvedTensions,
  sourceLabel,
  statusVariant,
  type Tension,
} from "./format";
import { ResolveDialog } from "./ResolveDialog";
import { RetractDialog } from "./RetractDialog";
import { SupersedeDialog } from "./SupersedeDialog";

type Dialog = { kind: "supersede" } | { kind: "retract" } | { kind: "resolve"; tension: Tension };

/** One current assertion: statement, rationale, who said it and where it came
 *  from, its open tensions (each with a Resolve), and the three things the
 *  user can do with it — change it (a new assertion superseding this one),
 *  retract it, or read how it got here. */
export function AssertionRow({
  assertion,
  graph,
  projectId,
}: {
  assertion: ContextAssertion;
  graph: ContextGraph;
  projectId: string;
}) {
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [showHistory, setShowHistory] = useState(false);
  const open = openTensions(graph, assertion.id);
  const resolved = resolvedTensions(graph, assertion.id);
  const history = showHistory ? historyOf(graph, assertion.id) : [];
  const settled = assertion.status === "retracted" || assertion.status === "abandoned";
  const hasHistory = !!assertion.supersedes || resolved.length > 0;

  return (
    <div className={`pc-assertion ${assertion.stance}`}>
      <div className="pc-statement text-sm">{assertion.statement}</div>
      {assertion.rationale && <div className="pc-meta text-xs">{assertion.rationale}</div>}
      <div className="pc-badges text-xs">
        <Badge variant={statusVariant(assertion.status)}>{assertion.status}</Badge>
        {assertion.stance === "rejected" && <Badge>rejected</Badge>}
        <Badge hint={sourceLabel(assertion.source)}>
          {provenanceLabel(assertion.author, assertion.provenance)} ·{" "}
          {sourceLabel(assertion.source)}
        </Badge>
        {open.length > 0 && (
          <Badge variant="warn" hint={open.map((t) => t.other?.statement ?? t.otherId).join("\n")}>
            contradicted
          </Badge>
        )}
        <span className="pc-actions">
          {!settled && (
            <Button variant="link" size="sm" onClick={() => setDialog({ kind: "supersede" })}>
              Change
            </Button>
          )}
          {!settled && (
            <Button variant="link" size="sm" danger onClick={() => setDialog({ kind: "retract" })}>
              Retract
            </Button>
          )}
          {hasHistory && (
            <Button variant="link" size="sm" onClick={() => setShowHistory((v) => !v)}>
              {showHistory ? "Hide history" : "History"}
            </Button>
          )}
        </span>
      </div>

      {open.map((t) => (
        <div key={t.otherId} className="pc-meta text-xs">
          with “{t.other?.statement ?? t.otherId}”{t.edge.reasoning && <> — {t.edge.reasoning}</>}{" "}
          <Button
            variant="link"
            size="sm"
            onClick={() => setDialog({ kind: "resolve", tension: t })}
          >
            Resolve
          </Button>
        </div>
      ))}

      {showHistory && (
        <div className="pc-history text-xs">
          {assertion.supersedes && (
            <span className="pc-meta">Replaced: {assertion.supersedes.reasoning}</span>
          )}
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
          {resolved.map((t) => (
            <div key={t.otherId}>
              <div>with “{t.other?.statement ?? t.otherId}”</div>
              <div className="pc-meta">{resolutionLabel(t.edge)}</div>
            </div>
          ))}
        </div>
      )}

      {dialog?.kind === "supersede" && (
        <SupersedeDialog
          assertion={assertion}
          projectId={projectId}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === "retract" && (
        <RetractDialog
          assertion={assertion}
          projectId={projectId}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === "resolve" && (
        <ResolveDialog
          assertion={assertion}
          tension={dialog.tension}
          projectId={projectId}
          onClose={() => setDialog(null)}
        />
      )}
    </div>
  );
}
