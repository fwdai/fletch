import { AccountAvatar } from "@/components/AccountAvatar";
import { Icon } from "@/components/Icon";
import type { AccountOption, AccountView } from "./useAccountView";

function optionNote(option: AccountOption): string | null {
  if (option.status === "signed_out") return "Signed out · sign in under Settings › Providers";
  return option.spent;
}

/** The account flyout's rows: every account of the agent's provider, the
 *  current one checked. A pick moves the agent onto it (a new session starts
 *  under it). Signed-out accounts can't be picked; one whose limit is spent
 *  can, with its reset shown, since that may be minutes away. */
export function AccountOptions({
  view,
  onPick,
  onManage,
}: {
  view: AccountView;
  onPick: (id: string) => void;
  onManage: () => void;
}) {
  return (
    <div className="model-list">
      {view.locked && (
        <div className="model-acct-note text-xs">Switch after this turn finishes</div>
      )}
      {view.options.map((o) => {
        const blocked = !o.current && (o.disabled || view.locked);
        const note = optionNote(o);
        return (
          <button
            key={o.id}
            type="button"
            role="menuitemradio"
            aria-checked={o.current}
            // aria-disabled, not the native attr: a disabled button swallows
            // hover in the WebView, and the row's title is how a long name
            // reads in full.
            aria-disabled={blocked}
            title={o.label}
            className={`model-option flex-center ${o.current ? "active" : ""} ${blocked ? "is-disabled" : ""}`}
            onClick={(e) => {
              e.stopPropagation();
              if (!blocked && !o.current) onPick(o.id);
            }}
          >
            <AccountAvatar id={o.id} label={o.label} size={18} spent={o.spent !== null} />
            <span className="model-option-main">
              <span className="model-option-name truncate text-base">{o.label}</span>
              {note && (
                <span className={`model-option-desc truncate text-xs ${o.spent ? "is-spent" : ""}`}>
                  {note}
                </span>
              )}
            </span>
            {o.current && <Icon name="check" size={13} />}
          </button>
        );
      })}
      <button
        type="button"
        className="model-option model-acct-manage flex-center"
        onClick={onManage}
      >
        <span className="model-option-main">
          <span className="model-option-name text-base">Manage accounts…</span>
        </span>
        <Icon name="settings" size={13} />
      </button>
    </div>
  );
}
