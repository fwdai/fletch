import { useState } from "react";
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
    <div className="fork-menu">
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
          <DropdownMenu className="fork-menu-dd">
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
