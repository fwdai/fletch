import { EnvironmentDot } from "@/components/HostChip";
import { Icon } from "@/components/Icon";
import { DropdownItem } from "@/components/ui/Dropdown";
import type { EnvironmentEntry } from "@/store/environments";
import type { RunTargetOption } from "./options";

/** One place in the menu: a paired host by its dot and name, This Mac by its
 *  own glyph. One that can't run the workspace is disabled, with why written
 *  under its name: a greyed place has to explain itself without a hover. */
export function TargetItem({
  option,
  env,
  checked,
  onPick,
}: {
  option: RunTargetOption;
  env: EnvironmentEntry | undefined;
  checked: boolean;
  onPick: () => void;
}) {
  return (
    <DropdownItem
      as="button"
      role="menuitemradio"
      aria-checked={checked}
      active={checked}
      className="target-item"
      disabled={!option.available}
      onClick={onPick}
    >
      <span className="di-i">
        {env?.kind === "remote" ? <EnvironmentDot env={env} /> : <Icon name="laptop" size={14} />}
      </span>
      <span className="target-item-text">
        <span className="di-l">{option.label}</span>
        {option.reason && <span className="target-item-why">{option.reason}</span>}
      </span>
      {checked && <Icon name="check" size={13} />}
    </DropdownItem>
  );
}
