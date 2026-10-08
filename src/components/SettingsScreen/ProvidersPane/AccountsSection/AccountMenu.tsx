import { DropdownItem } from "@/components/ui/Dropdown";
import { MenuButton } from "@/components/ui/MenuButton";
import type { AccountAction } from "./accountActions";

/** The three-dot menu in an account card's corner: its less frequent actions
 *  (sign out, delete), kept off the card itself. Picking one only closes the
 *  menu and hands it back — the card asks to confirm. Renders nothing for an
 *  account with no action to offer. */
export function AccountMenu({
  actions,
  disabled,
  onPick,
}: {
  actions: AccountAction[];
  disabled?: boolean;
  onPick: (action: AccountAction) => void;
}) {
  if (actions.length === 0) return null;
  return (
    <MenuButton icon="more" tip="Account actions" compact disabled={disabled}>
      {(close) =>
        actions.map((action) => (
          <DropdownItem
            key={action.id}
            as="button"
            role="menuitem"
            danger={action.danger}
            onClick={() => {
              close();
              onPick(action);
            }}
          >
            <span className="di-l">{action.label}</span>
          </DropdownItem>
        ))
      }
    </MenuButton>
  );
}
