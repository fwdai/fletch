// Board/InboxLane.tsx — captured rows waiting for (or done with) triage, above
// the Needs-you strip. Nothing here is on the roadmap: what triage makes of a
// row arrives as ghosts in the horizons below, ruled on with the usual bar.
// Mined brainstorms fold under a disclosure in the decision log's style. The
// lane renders nothing while the inbox is empty.

import { useState } from "react";
import type { RoadmapItem } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { type Inbox, inboxChip } from "../inbox";
import { InboxCard } from "./InboxCard";

export function InboxLane({
  inbox,
  triageOn,
  focusCode,
  onOpenInChat,
  onDiscard,
}: {
  inbox: Inbox;
  /** The project's `roadmap.triage` dial, for the chips' wording. */
  triageOn: boolean;
  focusCode?: string | null;
  onOpenInChat?: (item: RoadmapItem) => void;
  onDiscard?: (id: string) => void;
}) {
  const [minedOpen, setMinedOpen] = useState(false);
  const { open, mined } = inbox;
  if (open.length === 0 && mined.length === 0) return null;

  return (
    <section className="rm-inbox">
      <div className="rm-rail-h flex-center text-xs">
        <span className="rm-rail-n iflex-center mono">
          <Icon name="inbox" size={11} />
          Inbox
        </span>
        <span className="rm-rail-hint truncate">
          {open.length === 0
            ? "Everything captured has been mined."
            : `${open.length === 1 ? "One capture" : `${open.length} captures`} — nothing here is on the roadmap until you accept what the PM makes of it.`}
        </span>
      </div>
      {open.length > 0 && (
        <div className="rm-inbox-list">
          {open.map((entry) => (
            <InboxCard
              key={entry.item.id}
              item={entry.item}
              chip={inboxChip(entry, triageOn)}
              waiting={entry.waiting}
              focused={focusCode === entry.item.code}
              onOpenInChat={onOpenInChat && (() => onOpenInChat(entry.item))}
              onDiscard={onDiscard && (() => onDiscard(entry.item.id))}
            />
          ))}
        </div>
      )}
      {mined.length > 0 && (
        <div className="rm-inbox-mined">
          <button
            type="button"
            className="rm-nd-h flex-center text-xs"
            onClick={() => setMinedOpen(!minedOpen)}
            aria-expanded={minedOpen}
          >
            <Icon name="archive" size={11} />
            <span className="rm-nd-l mono">Mined</span>
            <span className="rm-nd-c mono">{mined.length}</span>
            <Icon name="chevD" size={9} className="rm-nd-chev" />
          </button>
          {minedOpen && (
            <ul className="rm-nd-list">
              {mined.map((entry) => (
                <li key={entry.item.id} className="rm-nd-row">
                  <div className="rm-nd-line flex-center">
                    <span className="rm-code mono text-xs">{entry.item.code}</span>
                    <span className="rm-nd-t text-sm truncate">{entry.item.title}</span>
                    <span className="grow" />
                    <span className="rm-nd-why text-xs">{inboxChip(entry, triageOn)}</span>
                    {onDiscard && (
                      <Button variant="ghost" size="sm" onClick={() => onDiscard(entry.item.id)}>
                        Discard
                      </Button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}
