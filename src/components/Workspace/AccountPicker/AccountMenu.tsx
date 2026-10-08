import { Icon } from "@/components/Icon";
import { ProviderAuthBadge } from "@/components/SettingsScreen/ProviderAuthBadge";
import { DropdownItem, DropdownSection, DropdownSeparator } from "@/components/ui/Dropdown";
import type { AccountChoice } from "./choices";

export function AccountMenu({
  choices,
  onPick,
  onManage,
}: {
  choices: AccountChoice[];
  onPick: (id: string) => void;
  onManage: () => void;
}) {
  return (
    <>
      <DropdownSection>Run the next turn as</DropdownSection>
      {choices.length === 0 && (
        <DropdownItem disabled>
          <span className="di-l">Loading accounts…</span>
        </DropdownItem>
      )}
      {choices.map((c) => (
        <DropdownItem
          key={c.id}
          as="button"
          role="menuitemradio"
          aria-checked={c.current}
          active={c.current}
          disabled={c.disabled}
          onClick={() => onPick(c.id)}
        >
          <span className="di-i">{c.current && <Icon name="check" size={12} />}</span>
          <span className="di-l">{c.label}</span>
          <ProviderAuthBadge status={c.status} detail={c.detail} />
        </DropdownItem>
      ))}
      <DropdownSeparator />
      <DropdownItem as="button" role="menuitem" onClick={onManage}>
        <span className="di-i">
          <Icon name="settings" size={12} />
        </span>
        <span className="di-l">Manage accounts…</span>
      </DropdownItem>
    </>
  );
}
