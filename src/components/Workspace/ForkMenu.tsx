import { useLayoutEffect, useRef, useState } from "react";
import type { ForkCode, ForkContext } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu } from "@/components/ui/Dropdown";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";

/** One entry in a fork menu: a label plus the Code × Context it forks with. */
export interface ForkOption {
  key: string;
  label: string;
  code: ForkCode;
  context: ForkContext;
}

/** Bottom edge the menu must fit within: the nearest scrolling ancestor (which
 *  clips absolutely-positioned descendants — e.g. the chat's `overflow-y: auto`
 *  scroller, below which the composer sits), clamped to the viewport. */
function clipBottom(el: HTMLElement): number {
  for (let node = el.parentElement; node; node = node.parentElement) {
    const overflowY = getComputedStyle(node).overflowY;
    if (overflowY === "auto" || overflowY === "scroll") {
      return Math.min(node.getBoundingClientRect().bottom, window.innerHeight);
    }
  }
  return window.innerHeight;
}

/** A split-icon button that opens a small menu of fork options and runs the
 *  chosen one (creating + opening the new workspace via the store). Shared by
 *  the turn-seam and workspace-header entry points, which differ only in their
 *  option list and `compact` sizing. */
export function ForkMenu({
  agentId,
  options,
  tip,
  compact = false,
}: {
  agentId: string;
  options: ForkOption[];
  tip: string;
  compact?: boolean;
}) {
  const forkAgent = useAppStore((s) => s.forkAgent);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  // Open down unless the trigger sits too close to the clip edge for the list
  // to fit below — then flip up. Measured after render, before paint.
  const [placement, setPlacement] = useState<"down" | "up">("down");
  const wrapRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    if (!open) return;
    const wrap = wrapRef.current;
    const menu = menuRef.current;
    if (!wrap || !menu) return;
    const trigger = wrap.getBoundingClientRect();
    const menuHeight = menu.offsetHeight;
    const spaceBelow = clipBottom(wrap) - trigger.bottom;
    const spaceAbove = trigger.top;
    // Flip up only when it doesn't fit below *and* there's more room above, so a
    // cramped-both-ways menu still opens down (its natural, expected direction).
    setPlacement(spaceBelow < menuHeight + 8 && spaceAbove > spaceBelow ? "up" : "down");
  }, [open]);

  const run = async (opt: ForkOption) => {
    setOpen(false);
    setBusy(true);
    try {
      await forkAgent(agentId, opt.code, opt.context);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fork-menu" ref={wrapRef}>
      <IconButton
        size={compact ? "xs" : undefined}
        tip={tip}
        className="turn-fork"
        aria-label={tip}
        disabled={busy}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name="split" size={compact ? 12 : undefined} />
      </IconButton>
      {open && (
        <>
          {/* Full-viewport scrim: any outside click dismisses the menu. */}
          <div className="fork-menu-scrim" onClick={() => setOpen(false)} />
          <DropdownMenu ref={menuRef} className={`fork-menu-dd ${placement}`}>
            {options.map((opt) => (
              <DropdownItem key={opt.key} as="button" onClick={() => run(opt)}>
                <span className="di-l">{opt.label}</span>
              </DropdownItem>
            ))}
          </DropdownMenu>
        </>
      )}
    </div>
  );
}
