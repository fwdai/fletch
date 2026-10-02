import { useState } from "react";
import type { ForkCode, ForkContext } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownSeparator } from "@/components/ui/Dropdown";
import { MenuButton } from "@/components/ui/MenuButton";
import { providerFor } from "@/helpers";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { ChoiceGroup } from "./ChoiceGroup";
import { codeChoices, contextChoices, DEFAULT_CODE, defaultContext } from "./options";

/** A split-icon button that opens the fork menu: pick how the new agent knows
 *  the conversation (context) and what its code starts from (code), then fork —
 *  creating and opening the new workspace via the store. Anchored on the turn
 *  `turnId` names (the menu under a turn) or, when null, on the whole
 *  conversation (the workspace header), which has no code of its own to go
 *  back to. */
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
  const provider = useAppStore((s) => providerFor(s, agentId));
  const [busy, setBusy] = useState(false);
  const [context, setContext] = useState<ForkContext>(() => defaultContext(provider));
  const [code, setCode] = useState<ForkCode>(DEFAULT_CODE);
  const scope = turnId === null ? "conversation" : "message";

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
            choices={contextChoices(scope, provider)}
            value={context}
            onChange={setContext}
          />
          <ChoiceGroup title="Code" choices={codeChoices(scope)} value={code} onChange={setCode} />
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
