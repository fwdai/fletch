import { useState } from "react";
import { api, type ContextGraph, type ContextProposal, type DismissReason } from "@/api";
import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { DismissAll } from "./DismissAll";
import {
  corroborationLabel,
  entityName,
  provenanceLabel,
  sortForReview,
  sourceLabel,
  summarizeProposal,
  unacceptedPending,
} from "./format";

const DISMISS_REASONS: DismissReason[] = ["wrong", "trivial", "duplicate", "already_known"];

/** The pending proposals — what agents, the extractor and the ingesters want
 *  to record, entities and assertions alike — each with its evidence and an
 *  Accept / Dismiss. */
export function ReviewQueue({
  proposals,
  graph,
  projectId,
}: {
  proposals: ContextProposal[];
  graph: ContextGraph;
  projectId: string;
}) {
  if (proposals.length === 0) {
    return <p className="pc-meta text-sm">Nothing to review.</p>;
  }
  return (
    <div className="pc-list">
      <DismissAll projectId={projectId} proposals={proposals} />
      {sortForReview(proposals).map((p) => (
        <ProposalCard key={p.id} proposal={p} graph={graph} projectId={projectId} />
      ))}
    </div>
  );
}

function ProposalCard({
  proposal,
  graph,
  projectId,
}: {
  proposal: ContextProposal;
  graph: ContextGraph;
  projectId: string;
}) {
  const [reason, setReason] = useState<DismissReason>("wrong");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { payload } = proposal;
  const relation = payload.type === "assertion" ? payload.relation : null;
  const target = relation?.target ? graph.assertions.find((a) => a.id === relation.target) : null;
  const pending = payload.type === "assertion" ? payload.about_pending : [];
  // A subject that is still only a proposal blocks Accept until it lands.
  const blocked = unacceptedPending(graph, pending);
  const detail = payload.type === "assertion" ? payload.input.rationale : payload.input.summary;
  const corroborated = corroborationLabel(proposal);

  const rule = (verdict: "accept" | "dismiss") => {
    setBusy(true);
    setError(null);
    api
      .contextRuleProposal(
        projectId,
        proposal.id,
        verdict,
        verdict === "dismiss" ? reason : undefined,
      )
      .catch((e) => {
        setError(String(e));
        setBusy(false);
      });
  };

  return (
    <div className="pc-proposal">
      <div className="text-sm">{summarizeProposal(proposal)}</div>
      {detail && <div className="pc-meta text-xs">{detail}</div>}
      <div className="pc-badges text-xs">
        <Badge>{payload.type}</Badge>
        <Badge hint={sourceLabel(payload.stamp.source)}>
          {provenanceLabel(payload.stamp.author, payload.stamp.provenance)} ·{" "}
          {sourceLabel(payload.stamp.source)}
        </Badge>
        {corroborated && <Badge>{corroborated}</Badge>}
        {payload.type === "assertion" && (payload.input.about.length > 0 || pending.length > 0) && (
          <span className="pc-meta">
            about {payload.input.about.map((id) => entityName(graph, id)).join(", ")}
            {payload.input.about.length > 0 && pending.length > 0 && ", "}
            {pending.map((slug, i) => (
              <span key={slug}>
                {i > 0 && ", "}
                <span className="pc-chip">{slug}</span> (pending entity)
              </span>
            ))}
          </span>
        )}
      </div>
      {relation && relation.kind !== "new" && (
        <div className="text-xs">
          <Badge variant={relation.kind === "contradicts" ? "warn" : "neutral"}>
            {relation.kind}
          </Badge>{" "}
          {target ? <span>{target.statement}</span> : <span className="pc-meta">(gone)</span>}
          {relation.reasoning && <span className="pc-meta"> — {relation.reasoning}</span>}
        </div>
      )}
      {proposal.evidence.map((ev, i) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: evidence only grows, and a run can repeat a quote
        <blockquote key={`${ev.turn_id ?? ""}:${i}`} className="pc-quote text-xs">
          {ev.quote}
        </blockquote>
      ))}
      <div className="pc-inline-form text-sm">
        <Button
          variant="primary"
          size="sm"
          disabled={busy || blocked.length > 0}
          onClick={() => rule("accept")}
        >
          Accept
        </Button>
        {blocked.length > 0 && (
          <span className="pc-meta text-xs">accept the entity `{blocked[0]}` first</span>
        )}
        <select
          className="ps-input text-sm"
          value={reason}
          onChange={(e) => setReason(e.target.value as DismissReason)}
          aria-label="Dismiss reason"
        >
          {DISMISS_REASONS.map((r) => (
            <option key={r} value={r}>
              {r.replace("_", " ")}
            </option>
          ))}
        </select>
        <Button variant="outline" size="sm" disabled={busy} onClick={() => rule("dismiss")}>
          Dismiss
        </Button>
      </div>
      {error && <div className="pc-error text-sm">{error}</div>}
    </div>
  );
}
