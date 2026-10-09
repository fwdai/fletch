import { Icon } from "@/components/Icon";
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
      <div className="model-sect flex-center text-xs">
        <span>{title}</span>
        <span className="model-sect-line" />
      </div>
      <button
        type="button"
        className={`model-option model-acct-row flex-center ${open ? "hot" : ""}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title={view.label}
        onMouseEnter={onOpen}
        onClick={onOpen}
      >
        <Icon
          name="user"
          size={13}
          className={`model-acct-glyph ${view.spent ? "is-spent" : ""}`}
        />
        <span className="model-option-main">
          <span className="model-option-name truncate text-base">{view.label}</span>
        </span>
        {view.spent && <span className="model-acct-spent text-xs">limit</span>}
        <Icon name="chevR" size={13} className="model-acct-chev" />
      </button>
    </>
  );
}
