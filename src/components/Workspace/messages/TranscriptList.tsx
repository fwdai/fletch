// The scrolling transcript itself: a log that follows its bottom and the turn
// navigator around one agent's rows (TranscriptRows). Shared by every surface
// that shows one agent's conversation: ChatView (which stacks a composer under
// it), a sub-agent thread and the roadmap's chat pane.
import { type MutableRefObject, useRef } from "react";
import type { AgentRecord } from "@/api";
import { useStickToBottom } from "@/util/stickToBottom";
import { ChatNav } from "../ChatNav";
import { RevealQuery } from "./Disclosure";
import { TranscriptRows } from "./TranscriptRows";
import type { Transcript } from "./useTranscript";

interface Props {
  agent: AgentRecord;
  transcript: Transcript;
  /** The agent is mid-turn: rows in the open turn may show a live spinner. */
  liveBusy: boolean;
  /** Quiet inline anchor for a just-sent first prompt with nothing back yet. */
  pending?: boolean;
  /** Lifted so a parent can wire find-in-conversation to the same container.
   *  Omit and the list owns its own ref. */
  scrollRef?: MutableRefObject<HTMLDivElement | null>;
  /** Filled with the log's "go to the bottom and follow", for a parent that
   *  sends a message while the user is scrolled up. */
  toBottomRef?: MutableRefObject<() => void>;
  /** Suppress the turn navigator, e.g. while find-in-conversation is open (it
   *  owns that corner). */
  hideNav?: boolean;
  /** The find-in-conversation query while the find bar is open. Folded rows
   *  holding a match open for as long as it stands. */
  revealQuery?: string;
}

export function TranscriptList({
  agent,
  transcript,
  liveBusy,
  pending,
  scrollRef,
  toBottomRef,
  hideNav,
  revealQuery = "",
}: Props) {
  const ownRef = useRef<HTMLDivElement | null>(null);
  const ref = scrollRef ?? ownRef;

  // Each conversation opens at its latest: the content column is keyed by the
  // agent, and the same key starts the follow over.
  const toBottom = useStickToBottom(ref, agent.id);
  if (toBottomRef) toBottomRef.current = toBottom;

  return (
    <div className="chat-scroll-wrap">
      <div className="chat-scroll" ref={ref}>
        <div className="chat-inner fade-in" key={agent.id}>
          <RevealQuery.Provider value={revealQuery.trim().toLowerCase()}>
            <TranscriptRows
              agent={agent}
              transcript={transcript}
              liveBusy={liveBusy}
              pending={pending}
            />
          </RevealQuery.Provider>
        </div>
      </div>
      {!hideNav && <ChatNav scrollRef={ref} turns={transcript.turns} />}
    </div>
  );
}
