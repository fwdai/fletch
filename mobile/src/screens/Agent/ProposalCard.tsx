import type { RoadmapItem } from "@desktop/api/types/roadmap";
import { Icon } from "@desktop/components/Icon";
import { useState } from "react";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";

/** How many acceptance criteria are worth the height inline. Past this the list
 *  is a line the user can open — a ghost is a decision, not a spec review. */
const CRITERIA_SHOWN = 3;

/** A ticket the PM has proposed during this planning chat: a real roadmap row
 *  parked at `proposed`, drawn where the idea was had so the user can rule on it
 *  without leaving the conversation.
 *
 *  Accepting is `proposed → open`, conditionally — the same transition the
 *  desktop board's accept makes — with "Add & start agent" queueing it for the
 *  drainer in the same gesture. Discarding deletes: the row never made the
 *  board, so there is no decision to keep. */
export function ProposalCard({ item }: { item: RoadmapItem }) {
  const accept = useStore((s) => s.acceptProposal);
  const discard = useStore((s) => s.discardProposal);
  // Only how long a tap takes: a failure is the store's to report (`lastError`),
  // and a success takes the card away with it.
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState(false);

  const rule = (run: () => Promise<void>) => {
    setBusy(true);
    void run()
      .catch(ignore)
      .finally(() => setBusy(false));
  };

  const criteria = open ? item.accept : item.accept.slice(0, CRITERIA_SHOWN);
  const hidden = item.accept.length - criteria.length;

  return (
    <div className="appr q prop rise">
      <div className="h">
        <Icon name="map" size={16} />
        <span className="mono">{item.code}</span>
        Proposed
      </div>
      <div className="title">{item.title}</div>
      <div className="why">{item.why}</div>
      {item.accept.length > 0 && (
        <>
          <ul className="crit">
            {criteria.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
          {item.accept.length > CRITERIA_SHOWN && (
            <button type="button" className="crit-more" onClick={() => setOpen(!open)}>
              <Icon name="chevD" size={13} style={open ? { transform: "rotate(180deg)" } : {}} />
              {open ? "Fewer criteria" : `${hidden} more criteria`}
            </button>
          )}
        </>
      )}
      <div className="acts">
        <button
          type="button"
          className="btn primary"
          disabled={busy}
          onClick={() => rule(() => accept(item, false))}
        >
          <Icon name="plus" size={15} strokeWidth={2.2} />
          Add to backlog
        </button>
        <button
          type="button"
          className="btn ghost"
          disabled={busy}
          onClick={() => rule(() => accept(item, true))}
        >
          <Icon name="play" size={15} strokeWidth={2.2} />
          Add &amp; start agent
        </button>
      </div>
      <button
        type="button"
        className="btn discard"
        disabled={busy}
        onClick={() => rule(() => discard(item))}
      >
        Discard
      </button>
    </div>
  );
}
