import { useState } from "react";
import type { ForkCode, ForkContext } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownSeparator } from "@/components/ui/Dropdown";
import { MenuButton } from "@/components/ui/MenuButton";
import { providerFor } from "@/helpers";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { ChoiceGroup } from "./ChoiceGroup";
import { CODE_CHOICES, contextChoices, DEFAULT_CODE, defaultContext } from "./options";

/** A split-icon button that opens the fork menu under a turn: pick how the new
 *  agent knows the conversation up to that turn (context) and what its code
 *  starts from (code), then fork — creating and opening the new workspace via
 *  the store. */
export function ForkMenu({
  agentId,
  turnId,
  tip,
  compact = false,
}: {
  agentId: string;
  turnId: string;
  tip: string;
  compact?: boolean;
}) {
  // `fork_agent` is not on a host's op table, so on a remote environment there
  // is nothing to offer.
  const forkGate = useGate("fork");
  const forkAgent = useAppStore((s) => s.forkAgent);
  const provider = useAppStore((s) => providerFor(s, agentId));
  const [busy, setBusy] = useState(false);
  const [context, setContext] = useState<ForkContext>(() => defaultContext(provider));
  const [code, setCode] = useState<ForkCode>(DEFAULT_CODE);

  const fork = async () => {
    setBusy(true);
    try {
      await forkAgent(agentId, turnId, code, context);
    } finally {
      setBusy(false);
    }
  };

  if (forkGate) return null;

  return (
    <MenuButton icon="split" tip={tip} compact={compact} className="turn-fork" disabled={busy}>
      {(close) => (
        <>
          <ChoiceGroup
            title="Context"
            choices={contextChoices(provider)}
            value={context}
            onChange={setContext}
          />
          <ChoiceGroup title="Code" choices={CODE_CHOICES} value={code} onChange={setCode} />
          <DropdownSeparator />
          <DropdownItem
            as="button"
            role="menuitem"
            onClick={() => {
              close();
              void fork();
            }}
          >
            <span className="di-i">
              <Icon name="split" size={12} />
            </span>
            <span className="di-l">Fork to new workspace</span>
          </DropdownItem>
        </>
      )}
    </MenuButton>
  );
}
