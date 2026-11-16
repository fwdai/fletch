import { AccountAvatar } from "@/components/AccountAvatar";
import { Icon } from "@/components/Icon";
import { FlyoutOption } from "@/components/ui/FlyoutMenu";
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
    <div className="fly-list">
      {view.locked && (
        <div className="model-acct-note text-xs">Switch after this turn finishes</div>
      )}
      {view.options.map((o) => {
        const blocked = !o.current && (o.disabled || view.locked);
        const note = optionNote(o);
        return (
          <FlyoutOption
            key={o.id}
            role="menuitemradio"
            aria-checked={o.current}
            // The row's title is how a long name reads in full.
            title={o.label}
            lead={<AccountAvatar id={o.id} label={o.label} spent={o.spent !== null} />}
            name={o.label}
            desc={note && o.spent ? <span className="model-acct-spent">{note}</span> : note}
            active={o.current}
            disabled={blocked}
            onClick={(e) => {
              e.stopPropagation();
              if (!blocked && !o.current) onPick(o.id);
            }}
          />
        );
      })}
      <FlyoutOption
        className="fly-manage"
        name="Manage accounts…"
        trail={<Icon name="settings" size={13} />}
        onClick={onManage}
      />
    </div>
  );
}
