import type { AccountLimits, ProviderAccount } from "@desktop/api/types/providers";
import { Icon } from "@desktop/components/Icon";
import { accountLabel } from "@desktop/data/providerAccounts";
import { useState } from "react";
import { Notice } from "../../components/ui/Notice";
import { useStore } from "../../store";
import { Limits } from "./Limits";

/** One account: the check that makes it the one every agent of the provider
 *  runs under, its name and sign-in state, and its plan limits beneath. The
 *  desktop's AccountRow without the actions a phone can't take — signing in
 *  and out, adding and deleting happen on the Mac, where the login PTY is.
 *
 *  Tapping a signed-in account that isn't active selects it; the host refuses
 *  a signed-out one, so that card is inert and its badge says why. */
export function AccountCard({
  account,
  limits,
  nowMs,
}: {
  account: ProviderAccount;
  limits: AccountLimits | undefined;
  nowMs: number;
}) {
  const setActive = useStore((s) => s.setActiveProviderAccount);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selectable = !account.active && account.status !== "signed_out" && !busy;
  const choose = async () => {
    setError(null);
    setBusy(true);
    try {
      await setActive(account.provider, account.managed ? account.id : null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`pv-acct${account.active ? " on" : ""}`}>
      <button
        type="button"
        className="row pv-acct-main"
        disabled={!selectable}
        aria-pressed={account.active}
        onClick={() => void choose()}
      >
        <span className={`pv-radio${account.active ? " on" : ""}`}>
          {account.active && <Icon name="check" size={13} strokeWidth={2.6} />}
        </span>
        <span className="main">
          <span className="lbl">{accountLabel(account)}</span>
          <span className="sub">
            {account.managed
              ? "Managed by Fletch"
              : "Your CLI's own login, shared with the terminal"}
          </span>
        </span>
        <AuthBadge status={account.status} busy={busy} />
      </button>
      <Limits account={account} row={limits} nowMs={nowMs} />
      {error && (
        <Notice tone="error" className="pv-acct-error" onDismiss={() => setError(null)}>
          {error}
        </Notice>
      )}
    </div>
  );
}

/** Whether the account is logged in, as the desktop's ProviderAuthBadge shows
 *  it. Nothing for `unknown`: the host only says that when it has no cheap
 *  check for the credential store, and a wrong "Not signed in" would send the
 *  user chasing a login they already have. */
function AuthBadge({ status, busy }: { status: ProviderAccount["status"]; busy: boolean }) {
  if (busy) return <span className="pv-badge">Switching…</span>;
  if (status === "signed_in") return <span className="pv-badge ok">Signed in</span>;
  if (status === "signed_out") return <span className="pv-badge warn">Not signed in</span>;
  return null;
}
