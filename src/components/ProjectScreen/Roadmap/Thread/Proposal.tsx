import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import type { ProposalChange } from "../types";

const HEADLINE = {
  accepted: "Added to the roadmap",
  discarded: "Discarded",
  open: "Proposed changes",
} as const;

/** The commit point: everything the PM wants to change to the board, and the
 *  two buttons that make it real or drop it. Until one is pressed the board
 *  only shows these as ghosts.
 *
 *  Currently unmounted: the thread is a real agent transcript now, and the PM
 *  has no way to raise a proposal until the `roadmap_propose` RPC tool lands in
 *  the next slice. That slice renders this card inline in the transcript and
 *  wires its buttons to `useRoadmap.applyChanges` — the accept path it was
 *  built against, and the reason it stays here rather than being deleted and
 *  re-invented. */
export function Proposal({
  note,
  changes,
  resolved,
  onAccept,
  onDiscard,
}: {
  note: string;
  changes: ProposalChange[];
  resolved: "accepted" | "discarded" | null | undefined;
  onAccept: () => void;
  onDiscard: () => void;
}) {
  return (
    <div className={`rm-prop ${resolved ?? ""}`}>
      <div className="rm-prop-h">
        <span className="rm-prop-l mono text-xs">{HEADLINE[resolved ?? "open"]}</span>
        <span className="rm-prop-n mono text-xs">{note}</span>
      </div>

      <div className="rm-prop-list">
        {changes.map((c) =>
          c.kind === "add" ? (
            <div key={c.item.code} className="rm-prop-row flex-center text-sm">
              <span className="rm-prop-op add iflex-center mono">+</span>
              {/* No code yet — the backend allocates one if this is accepted. */}
              <span className="rm-prop-code mono text-xs">NEW</span>
              <span className="rm-prop-title truncate">{c.item.title}</span>
              <span className="rm-prop-tag mono text-xs">{c.item.horizon}</span>
            </div>
          ) : (
            <div key={c.code} className="rm-prop-row flex-center text-sm">
              <span className="rm-prop-op move iflex-center">
                <Icon name="arrowUp" size={10} />
              </span>
              <span className="rm-prop-code mono text-xs">{c.code}</span>
              <span className="rm-prop-title truncate">{c.why}</span>
              <span className="rm-prop-tag mono text-xs">
                {c.from} → {c.to}
              </span>
            </div>
          ),
        )}
      </div>

      {!resolved && (
        <div className="rm-prop-foot flex-center">
          <span className="rm-prop-hint text-xs">Nothing changes until you say so.</span>
          <span className="grow" />
          <Button variant="ghost" size="sm" onClick={onDiscard}>
            Discard
          </Button>
          <Button variant="primary" size="sm" onClick={onAccept}>
            <Icon name="check" size={11} /> Add to roadmap
          </Button>
        </div>
      )}
    </div>
  );
}
