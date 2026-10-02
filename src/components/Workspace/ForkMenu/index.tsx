import { useRef, useState } from "react";
import type { ForkCode, ForkContext } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSeparator } from "@/components/ui/Dropdown";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { ChoiceGroup } from "./ChoiceGroup";
import { codeChoices, contextChoices, DEFAULT_CODE, DEFAULT_CONTEXT } from "./options";
import { usePlacement } from "./usePlacement";

/** A split-icon button that opens the fork menu: pick what the new agent knows
 *  (context) and what its code starts from (code), then fork — creating and
 *  opening the new workspace via the store. Anchored on the turn `turnId`
 *  names (the menu under a turn) or, when null, on the whole conversation (the
 *  workspace header), which has no code of its own to go back to. */
export function ForkMenu({
  agentId,
  turnId,
  tip,
  compact = false,
}: {
  agentId: string;
  turnId: string | null;
  tip: string;
  compact?: boolean;
}) {
  // `fork_agent` is not on a host's op table, so on a remote environment there
  // is nothing to offer. Gated here rather than at the two call sites (the
  // workspace header and each turn footer) because this is where they meet.
  const forkGate = useGate("fork");
  const forkAgent = useAppStore((s) => s.forkAgent);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [context, setContext] = useState<ForkContext>(DEFAULT_CONTEXT);
  const [code, setCode] = useState<ForkCode>(DEFAULT_CODE);
  const wrapRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const placement = usePlacement(open, wrapRef, menuRef);
  const scope = turnId === null ? "conversation" : "message";

  const fork = async () => {
    setOpen(false);
    setBusy(true);
    try {
      await forkAgent(agentId, turnId, code, context);
    } finally {
      setBusy(false);
    }
  };

  if (forkGate) return null;

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
          <DropdownMenu ref={menuRef} role="menu" className={`fork-menu-dd ${placement}`}>
            <ChoiceGroup
              title="Context"
              choices={contextChoices(scope)}
              value={context}
              onChange={setContext}
            />
            <ChoiceGroup
              title="Code"
              choices={codeChoices(scope)}
              value={code}
              onChange={setCode}
            />
            <DropdownSeparator />
            <DropdownItem as="button" role="menuitem" onClick={fork}>
              <span className="di-i">
                <Icon name="split" size={12} />
              </span>
              <span className="di-l">Fork to new workspace</span>
            </DropdownItem>
          </DropdownMenu>
        </>
      )}
    </div>
  );
}
