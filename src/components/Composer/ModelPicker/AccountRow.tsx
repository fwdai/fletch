import { AccountAvatar } from "@/components/AccountAvatar";
import { Icon } from "@/components/Icon";
import { FlyoutOption, FlyoutSection } from "@/components/ui/FlyoutMenu";
import type { AccountView } from "./useAccountView";

/** The menu's first section: the account the agent runs under, as a row the
 *  height of a model row whose hover (or click) opens the account flyout
 *  beside the card. */
export function AccountRow({
  view,
  title,
  open,
  onOpen,
}: {
  view: AccountView;
  title: string;
  /** The account flyout is the one showing. */
  open: boolean;
  onOpen: () => void;
}) {
  return (
    <>
      <FlyoutSection>{title}</FlyoutSection>
      <FlyoutOption
        className="model-acct-row"
        aria-haspopup="menu"
        aria-expanded={open}
        title={view.label}
        lead={<AccountAvatar id={view.id} label={view.label} spent={view.spent} />}
        name={view.label}
        trail={
          <>
            {view.spent && <span className="model-acct-spent text-xs">limit</span>}
            <Icon name="chevR" size={13} className="model-acct-chev" />
          </>
        }
        hot={open}
        onMouseEnter={onOpen}
        onClick={onOpen}
      />
    </>
  );
}
