import { useCallback, useEffect, useRef, useState } from "react";
import type { AgentRecord } from "@/api";
import { useAppStore } from "@/store";
import { ChatComposer } from "./ChatComposer";
import { ChatSearch } from "./ChatSearch";
import { TranscriptList } from "./messages/TranscriptList";
import { isTurnPending } from "./messages/turnPending";
import { useTranscript } from "./messages/useTranscript";
import { useLiveBusy } from "./useLiveBusy";

/** Custom-view body: scrolling chat log + composer at the bottom.
 *  The composer here dispatches the user's message via the store; it
 *  doesn't care about provider routing yet. */
export function ChatView({ agent }: { agent: AgentRecord }) {
  const turnStartedAt = useAppStore((s) => s.turnStartedAt[agent.id]);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  // Owned here so sending a message can re-pin the log to the bottom.
  const pinnedToBottom = useRef(true);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");

  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    setSearchQuery("");
  }, []);

  // ⌘F / Ctrl+F opens find-in-conversation. A repeat press while open just
  // refocuses + selects the existing input (the bar is already mounted), which
  // mirrors how browsers behave.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "f" || e.key === "F")) {
        // The right-panel terminal has its own ⌘F (handled by xterm); its
        // keydown still bubbles to window, so ignore presses originating there.
        if ((e.target as HTMLElement | null)?.closest(".term-panel")) return;
        e.preventDefault();
        setSearchOpen(true);
        requestAnimationFrame(() => {
          const el = document.getElementById("chat-search-input") as HTMLInputElement | null;
          el?.focus();
          el?.select();
        });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Close the find bar when switching conversations — its matches belong to the
  // log we're leaving.
  useEffect(() => {
    setSearchOpen(false);
    setSearchQuery("");
  }, [agent.id]);

  // Log derivation (lazy history load, display policy, tool pairing, per-turn
  // bookkeeping) is shared with the native view's rail — see useTranscript.
  const transcript = useTranscript(agent);
  const { items, turns, activeModel, awaitingInput, openTurnStartedAt } = transcript;

  // Debounced "is working" (see useLiveBusy), shared with the Roadmap tab's PM
  // chat so both surfaces settle on the same beat.
  const liveBusy = useLiveBusy(agent.id, awaitingInput);

  // Phase A: user just sent a turn-starting prompt and nothing has landed yet.
  // A quiet inline anchor (dots only — label lives in the bottom status strip).
  // Only on the first turn: for later turns the chat already has content above
  // and the bottom status strip carries the "is working" signal, so the inline
  // anchor is redundant.
  const turnPending = liveBusy && isTurnPending(items) && turns.length <= 1;

  // Live-timer anchor: the backend's turn-start timestamp (from `turn:started`,
  // the same value the footer's duration uses, so they never drift). On reload
  // mid-turn no event fired this session, so fall back to the open turn's
  // persisted start. Absent during spawn → strip shows, timer waits.
  const liveStartedAt = liveBusy ? (turnStartedAt ?? openTurnStartedAt) : undefined;

  return (
    <div className="chat">
      {searchOpen && (
        <ChatSearch
          containerRef={scrollRef}
          query={searchQuery}
          onQueryChange={setSearchQuery}
          contentVersion={items}
          onClose={closeSearch}
        />
      )}
      <TranscriptList
        agent={agent}
        transcript={transcript}
        liveBusy={liveBusy}
        pending={turnPending}
        scrollRef={scrollRef}
        pinRef={pinnedToBottom}
        hideNav={searchOpen}
      />
      <ChatComposer
        agent={agent}
        activeModel={activeModel}
        liveBusy={liveBusy}
        liveStartedAt={liveStartedAt}
        onSend={() => {
          pinnedToBottom.current = true;
        }}
      />
    </div>
  );
}
