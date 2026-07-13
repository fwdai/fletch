// RunView/Timeline.tsx — the journal timeline (spec §14.2). A virtualized list
// over uniform rows (fixed height, so windowing stays simple and correct) fed by
// the paged journal + live appends. Rows render product-language summaries only;
// the raw payload JSON is available behind an expand affordance — a footer drawer
// that keeps the row heights uniform (the guardrail: no raw JSON inline).

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { WfEvent } from "../../../api";
import { Icon } from "../../../components/Icon";
import { summarizeEvent } from "../eventSummary";

const ROW_H = 34;
const OVERSCAN = 8;

export function Timeline({ events }: { events: WfEvent[] }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(0);
  const [openSeq, setOpenSeq] = useState<number | null>(null);
  // Whether the view is pinned to the bottom (so live appends keep following).
  const stuck = useRef(true);

  // Measure the viewport height (windowing needs it) and keep it current.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () => setViewport(el.clientHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Follow the tail when new events arrive and the user hasn't scrolled up.
  // biome-ignore lint/correctness/useExhaustiveDependencies: events.length is the intended re-run trigger, not an unused dep
  useEffect(() => {
    const el = scrollRef.current;
    if (el && stuck.current) el.scrollTop = el.scrollHeight;
  }, [events.length]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    stuck.current = el.scrollHeight - el.scrollTop - el.clientHeight < ROW_H * 2;
  };

  const total = events.length;
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const visibleCount = Math.ceil(viewport / ROW_H) + OVERSCAN * 2;
  const last = Math.min(total, first + visibleCount);
  const slice = events.slice(first, last);
  const openEvent = openSeq != null ? events.find((e) => e.seq === openSeq) : undefined;

  if (total === 0) {
    return (
      <div className="wf-tl-empty">
        <Icon name="clock" size={15} />
        <span>No activity yet — the timeline fills as the run progresses.</span>
      </div>
    );
  }

  return (
    <div className="wf-tl">
      <div className="wf-tl-scroll" ref={scrollRef} onScroll={onScroll}>
        <div className="wf-tl-spacer" style={{ height: total * ROW_H }}>
          {slice.map((ev, i) => {
            const idx = first + i;
            const s = summarizeEvent(ev);
            return (
              <button
                type="button"
                key={ev.seq}
                className={`wf-tl-row ${openSeq === ev.seq ? "open" : ""}`}
                style={{ top: idx * ROW_H, height: ROW_H }}
                onClick={() => setOpenSeq(openSeq === ev.seq ? null : ev.seq)}
                title="Show event details"
              >
                <span className="wf-tl-icon" style={{ color: s.tone }}>
                  <Icon name={s.icon} size={12} />
                </span>
                <span className="wf-tl-title">{s.title}</span>
                {s.detail && <span className="wf-tl-detail">{s.detail}</span>}
                <span className="wf-tl-time">{clock(ev.ts)}</span>
              </button>
            );
          })}
        </div>
      </div>

      {openEvent && (
        <div className="wf-tl-drawer">
          <div className="wf-tl-drawer-head">
            <span className="mono">{openEvent.type}</span>
            <button
              type="button"
              className="wf-tl-drawer-close"
              onClick={() => setOpenSeq(null)}
              aria-label="Close details"
            >
              <Icon name="close" size={12} />
            </button>
          </div>
          <pre className="wf-tl-drawer-body mono">{prettyPayload(openEvent.payload)}</pre>
        </div>
      )}
    </div>
  );
}

/** Wall-clock HH:MM:SS from an epoch-ms timestamp. */
function clock(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString(undefined, { hour12: false });
}

function prettyPayload(payload: unknown): string {
  try {
    return JSON.stringify(payload, null, 2);
  } catch {
    return String(payload);
  }
}
