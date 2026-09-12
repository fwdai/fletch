import { type ReactNode, useRef } from "react";
import { useSwipe } from "../../lib/swipe";
import { type NavItem, useStore } from "../../store";

/** How far in from the left a swipe-back may begin, like the iOS edge pan.
 *  Wide enough to catch a thumb, narrow enough to leave the transcript's
 *  code blocks their own horizontal scroll. */
const BACK_EDGE = 28;

/** The pushed screens, top-most last, with the edge swipe that pops them. */
export function Stack({
  dimmed,
  render,
}: {
  dimmed: boolean;
  render: (item: NavItem) => ReactNode;
}) {
  const nav = useStore((s) => s.nav);
  const sheet = useStore((s) => s.sheet);
  const pop = useStore((s) => s.pop);
  const ref = useRef<HTMLDivElement>(null);

  const live = nav.filter((i) => i.phase !== "leave");
  const topKey = live[live.length - 1]?.key;
  useSwipe(ref, {
    axis: "x",
    edge: BACK_EDGE,
    enabled: live.length > 1 && !sheet?.open,
    onCommit: pop,
  });

  return (
    <div ref={ref} className={`stack${dimmed ? " dimmed" : ""}`}>
      {nav.map((item) => {
        const cls =
          item.phase === "enter"
            ? "enter"
            : item.phase === "leave"
              ? "leave"
              : item.key === topKey
                ? "top"
                : "under";
        return (
          <div key={item.key} className={`scr ${cls}`}>
            {render(item)}
            <div className="scrim" />
          </div>
        );
      })}
    </div>
  );
}
