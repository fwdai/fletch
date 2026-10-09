import { Icon } from "@/components/Icon";
import type { AccountOption, AccountView } from "./useAccountView";

function optionTip(option: AccountOption): string | undefined {
  if (option.status === "signed_out") return "Signed out · sign in under Settings › Providers";
  return option.spent ?? undefined;
}

/** The menu's account section: one pill per account of the agent's provider,
 *  the current one selected. A pick moves the agent onto that account (a new
 *  session starts under it). Signed-out accounts can't be picked; one whose
 *  limit is spent can, with a warning, since its reset may be minutes away. */
export function AccountStrip({
  view,
  title,
  onPick,
  onManage,
  onMouseEnter,
}: {
  view: AccountView;
  title: string;
  onPick: (id: string) => void;
  onManage: () => void;
  onMouseEnter?: () => void;
}) {
  return (
    <div onMouseEnter={onMouseEnter}>
      <div className="model-sect flex-center text-xs">
        <span>{title}</span>
        <span className="model-sect-line" />
        <button
          type="button"
          className="model-sect-act flex-center"
          title="Manage accounts"
          aria-label="Manage accounts"
          onClick={onManage}
        >
          <Icon name="settings" size={11} />
        </button>
      </div>
      <div className="acct-strip" role="radiogroup" aria-label="Account">
        {view.options.map((o) => {
          const blocked = !o.current && (o.disabled || view.locked);
          const tip = optionTip(o);
          return (
            <button
              key={o.id}
              type="button"
              role="radio"
              aria-checked={o.current}
              // aria-disabled, not the native attr: a disabled button swallows
              // hover in the WebView and its tooltip would never show.
              aria-disabled={blocked}
              data-tip={tip}
              className={`acct-pill flex-center text-sm ${o.current ? "active" : ""} ${o.spent ? "is-spent" : ""} ${blocked ? "is-disabled" : ""} ${tip ? "tip" : ""}`}
              onClick={() => !blocked && !o.current && onPick(o.id)}
            >
              {o.spent && <span className="acct-pill-dot" />}
              <span className="truncate">{o.label}</span>
            </button>
          );
        })}
      </div>
      {view.locked && (
        <div className="acct-strip-note text-xs">Switch after this turn finishes</div>
      )}
    </div>
  );
}
