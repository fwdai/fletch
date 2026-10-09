import { Icon } from "@/components/Icon";
import type { AccountView } from "./useAccountView";

/** The menu's first section: the account the agent runs under, as a row whose
 *  hover (or click) opens the account flyout beside the card. */
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
        className={`model-agent-row flex-center ${open ? "hot" : ""}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title={view.label}
        onMouseEnter={onOpen}
        onClick={onOpen}
      >
        <span className={`model-acct-icon flex-center ${view.spent ? "is-spent" : ""}`}>
          <Icon name="user" size={14} />
        </span>
        <span className="model-agent-name truncate text-base">{view.label}</span>
        {view.spent && <span className="model-agent-ver model-acct-spent text-xs">limit</span>}
        <Icon name="chevR" size={12} />
      </button>
    </>
  );
}
