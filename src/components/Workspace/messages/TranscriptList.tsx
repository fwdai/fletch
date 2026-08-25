// The scrolling transcript itself: bottom-pinning and the turn navigator around
// one agent's rows (TranscriptRows). Shared by the custom view's ChatView (which
// stacks a composer under it) and the native view's rail (which sits beside the
// terminal).
import { type MutableRefObject, useEffect, useRef } from "react";
import type { AgentRecord } from "@/api";
import { ChatNav } from "../ChatNav";
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
  /** Lifted bottom-pin flag, so a parent that sends a message can re-pin the
   *  log to the bottom the way an in-list scroll would. */
  pinRef?: MutableRefObject<boolean>;
  /** Suppress the turn navigator — while find-in-conversation is open (it owns
   *  that corner), or in the native rail, which is too narrow for it. */
  hideNav?: boolean;
}

export function TranscriptList({
  agent,
  transcript,
  liveBusy,
  pending,
  scrollRef,
  pinRef,
  hideNav,
}: Props) {
  const { turns, transcriptLoading, log } = transcript;

  const ownRef = useRef<HTMLDivElement | null>(null);
  const ref = scrollRef ?? ownRef;

  // Whether the log is "pinned" to the bottom. While true we follow new
  // messages; once the user scrolls up we stop auto-scrolling and leave their
  // position alone until they scroll back down to the bottom.
  const ownPin = useRef(true);
  const pinnedToBottom = pinRef ?? ownPin;

  // Re-pin whenever we switch agents, so each conversation opens at its latest.
  useEffect(() => {
    pinnedToBottom.current = true;
  }, [agent.id, pinnedToBottom]);

  const handleScroll = () => {
    const el = ref.current;
    if (!el) return;
    // Allow a small slop so the user counts as "at the bottom" even a few
    // pixels short — sub-pixel rounding otherwise makes exact equality flaky.
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    pinnedToBottom.current = distanceFromBottom <= 40;
  };

  // biome-ignore lint/correctness/useExhaustiveDependencies: `log` is the change
  // signal rather than a value the body reads — new content arriving is exactly
  // when a bottom-pinned log has to follow.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (transcriptLoading) return;
    if (!pinnedToBottom.current) return;
    el.scrollTop = el.scrollHeight;
  }, [log, transcriptLoading, ref, pinnedToBottom]);

  return (
    <div className="chat-scroll-wrap">
      <div className="chat-scroll" ref={ref} onScroll={handleScroll}>
        <div className="chat-inner fade-in" key={agent.id}>
          <TranscriptRows
            agent={agent}
            transcript={transcript}
            liveBusy={liveBusy}
            pending={pending}
          />
        </div>
      </div>
      {!hideNav && <ChatNav scrollRef={ref} turns={turns} />}
    </div>
  );
}
