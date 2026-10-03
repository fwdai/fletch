import type { AgentRecord, RestoreReport, RewindScope } from "@/api";
import { DropdownItem, DropdownSection } from "@/components/ui/Dropdown";
import { rewindBlocker, rewindOptions } from "./options";
import { useCodeState } from "./useCodeState";

/** The rewind menu's rows, each disabled with its reason when it can't be
 *  picked. Mounted as the menu opens, so the code's preview is fresh; it is
 *  handed on with a code option for the confirmation. */
export function RewindOptions({
  agent,
  turnId,
  onPick,
}: {
  agent: AgentRecord;
  turnId: string;
  onPick: (scope: RewindScope, code: RestoreReport | null) => void;
}) {
  const blocker = rewindBlocker(agent);
  const code = useCodeState(agent.id, turnId, blocker === null);
  const report = typeof code === "object" && "report" in code ? code.report : null;

  return (
    <>
      <DropdownSection>Rewind to before this message</DropdownSection>
      {rewindOptions(blocker, code).map((option) => (
        <DropdownItem
          key={option.scope}
          as="button"
          role="menuitem"
          disabled={option.reason !== null}
          onClick={() => onPick(option.scope, option.scope === "conversation" ? null : report)}
        >
          <span className="di-l">
            {option.label}
            {option.reason && <span className="rewind-why text-xs">{option.reason}</span>}
          </span>
        </DropdownItem>
      ))}
    </>
  );
}
