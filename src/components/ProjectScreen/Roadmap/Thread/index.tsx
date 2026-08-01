import { useEffect, useRef } from "react";
import { Icon } from "@/components/Icon";
import type { RoadmapState } from "../useRoadmap";
import { Composer } from "./Composer";
import { Message, Thinking } from "./Message";

/** The left column: the conversation with the project's PM agent. It reads the
 *  repo, asks when a decision is genuinely open, and proposes board changes —
 *  it never edits the board itself. */
export function Thread({ roadmap }: { roadmap: RoadmapState }) {
  const { messages, thinking, blocked, suggestions, send } = roadmap;
  const scroll = useRef<HTMLDivElement>(null);

  // Beats arrive on a timer, so pin to the bottom as the thread grows.
  // biome-ignore lint/correctness/useExhaustiveDependencies: the deps are the intended re-run triggers, not unused reads
  useEffect(() => {
    const el = scroll.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages.length, thinking]);

  return (
    <section className="rm-thread">
      <div className="rm-thread-scroll" ref={scroll}>
        <div className="rm-thread-col">
          <div className="rm-pm flex-center">
            <span className="rm-pm-badge iflex-center">
              <Icon name="sparkle" size={13} />
            </span>
            <div>
              <div className="rm-pm-n text-sm">Project manager</div>
              <div className="rm-pm-s text-xs">
                Keeps the roadmap. Reads the repo before it writes anything down.
              </div>
            </div>
          </div>

          {messages.map((m) => (
            <Message key={m.id} msg={m} roadmap={roadmap} />
          ))}

          {thinking && <Thinking body="Working through it" />}
        </div>
      </div>

      <Composer blocked={blocked} suggestions={suggestions} onSend={send} />
    </section>
  );
}
