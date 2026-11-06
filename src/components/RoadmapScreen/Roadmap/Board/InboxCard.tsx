// Board/InboxCard.tsx — one captured row in the Inbox lane: the text, where
// triage stands, and the two things to do with it. Drawn as a ghost card (the
// row is not on the roadmap) with the card's own DecisionBar.

import type { RoadmapItem } from "@/api";
import { DecisionBar } from "./DecisionBar";

export function InboxCard({
  item,
  chip,
  waiting,
  focused,
  onOpenInChat,
  onDiscard,
}: {
  item: RoadmapItem;
  /** Where triage stands, from `inboxChip`. */
  chip: string;
  /** The host's note on why triage hasn't started, as the chip's tooltip. */
  waiting?: string | null;
  focused?: boolean;
  /** Start a planning chat on this row. Absent on a read-only board. */
  onOpenInChat?: () => void;
  /** Delete the row, as discarding a ghost does. Absent on a read-only board. */
  onDiscard?: () => void;
}) {
  const idea = item.kind === "idea" ? item.triage : null;
  return (
    <div className={`rm-item ghost ${focused ? "focus" : ""}`}>
      <div className="rm-item-h flex-center">
        <span className="rm-code mono text-xs">{item.code}</span>
        <span className="rm-title text-sm truncate">{item.title}</span>
        <span className="rm-state iflex-center text-xs st-q" title={waiting ?? undefined}>
          {chip}
        </span>
      </div>
      {/* An idea is shown open — its brief and questions are what the user
          answers; a raw capture shows the rest of what was written. */}
      {idea ? (
        <div className="rm-item-body">
          {idea.brief && <p className="rm-why text-sm">{idea.brief}</p>}
          {!!idea.questions?.length && (
            <ul className="rm-accept text-sm">
              {idea.questions.map((q) => (
                <li key={q}>{q}</li>
              ))}
            </ul>
          )}
        </div>
      ) : (
        item.why && (
          <div className="rm-item-body">
            <p className="rm-why text-sm">{item.why}</p>
          </div>
        )
      )}
      {(onOpenInChat || onDiscard) && (
        <DecisionBar
          label={idea ? "Idea — answer it with the PM" : "Captured — not on the roadmap yet"}
          acceptLabel="Open in PM chat"
          acceptIcon="notebookPen"
          declineLabel="Discard"
          onAccept={onOpenInChat}
          onDecline={onDiscard}
        />
      )}
    </div>
  );
}
