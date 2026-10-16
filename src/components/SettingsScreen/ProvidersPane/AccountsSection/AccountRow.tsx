import { useCallback, useState } from "react";
import type { AccountLimits, ProviderAccount } from "@/api/types/providers";
import { Icon } from "@/components/Icon";
import { ProviderAuthBadge } from "@/components/SettingsScreen/ProviderAuthBadge";
import { ProviderLoginTerminal } from "@/components/SettingsScreen/ProviderLogin";
import { Button } from "@/components/ui/Button";
import { IconButton } from "@/components/ui/IconButton";
import { Spinner } from "@/components/ui/Spinner";
import { accountLabel } from "@/data/providerAccounts";
import { loginCommand } from "@/data/providerDetail";
import type { ProviderId } from "@/data/providers";
import { useAppStore } from "@/store";
import { AccountMenu } from "./AccountMenu";
import { type AccountAction, accountActions } from "./accountActions";
import { LimitsPanel } from "./LimitsPanel";
import { agoLabel } from "./limitsFormat";

/** One account: the radio that makes it the one every agent runs under, its
 *  name, a Refresh for its own limits and how old the reading is, Sign in when
 *  it needs one, and a corner menu for the rest (re-authenticate, sign out,
 *  delete — see `accountActions`), the destructive ones confirmed inline. An
 *  account listed here is signed in unless it says otherwise: only a
 *  signed-out one wears a badge. Sign in and re-authenticate open the same
 *  embedded terminal as a single-login provider, run against this account's
 *  directory. Under it, the account's plan limits. */
export function AccountRow({
  account,
  providerId,
  providerLabel,
  signingIn,
  refreshingAll,
  onSignIn,
  onCloseSignIn,
  limits,
  nowMs,
}: {
  account: ProviderAccount;
  providerId: ProviderId;
  providerLabel: string;
  signingIn: boolean;
  /** The section's Refresh limits is running over every account, this one
   *  included: its own Refresh spins along and takes no second press. */
  refreshingAll: boolean;
  limits: AccountLimits | undefined;
  nowMs: number;
  onSignIn: () => void;
  onCloseSignIn: () => void;
}) {
  const setActive = useAppStore((s) => s.setActiveProviderAccount);
  const remove = useAppStore((s) => s.removeProviderAccount);
  const signOut = useAppStore((s) => s.signOutProviderAccount);
  const refreshLimits = useAppStore((s) => s.refreshProviderLimits);
  // Stable (a zustand action), so the terminal's one-shot outcome effect
  // isn't re-armed by a parent re-render — same contract as SignInSection.
  const refreshAccounts = useAppStore((s) => s.refreshProviderAccounts);
  const [confirming, setConfirming] = useState<AccountAction | null>(null);
  const [busy, setBusy] = useState(false);
  const [refreshingOwn, setRefreshingOwn] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const command = loginCommand(providerId);
  const inputId = `account-${providerId}-${account.id}`;
  const accountId = account.managed ? account.id : undefined;
  // Offered unless the account is known to be signed in — or a limits refresh
  // just found it signed out, whose hint below points at this button. While
  // the card offers Sign in, the menu doesn't also offer Re-authenticate.
  const needsSignIn = account.status !== "signed_in" || limits?.refresh?.status === "signed_out";
  const actions = accountActions(account, providerLabel, !!command && !needsSignIn);

  const choose = () => {
    setError(null);
    setActive(providerId, accountId ?? null).catch((err) => setError(String(err)));
  };
  // An action with a question waits for its answer; the rest run at once.
  const pick = (action: AccountAction) => {
    if (action.confirm) setConfirming(action);
    else if (action.id === "reauth") onSignIn();
  };
  // On success the re-list replaces this card (delete) or its badge (sign
  // out); either way the question is answered.
  const run = async (action: AccountAction) => {
    setError(null);
    setBusy(true);
    try {
      await (action.id === "delete" ? remove : signOut)(providerId, account.id);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
      setConfirming(null);
    }
  };

  // This account's limits alone. A signed-out account can't be read (limits
  // come with its own login), so the button waits for Sign in. The engine
  // holds back an account inside its refresh floor or a 429 back-off and
  // answers with the stored row, so pressing again is harmless.
  const refreshing = refreshingAll || refreshingOwn;
  const refreshOwn = async () => {
    setError(null);
    setRefreshingOwn(true);
    try {
      await refreshLimits(providerId, account.id);
    } catch (err) {
      setError(String(err));
    } finally {
      setRefreshingOwn(false);
    }
  };
  // Beside the name: what the default account is, and how old the reading
  // the meters show is — the thing Refresh changes, next to the button.
  const asOf = limits?.limits?.as_of;
  const sub = [
    account.managed ? null : "Your CLI's own login, shared with the terminal",
    asOf === undefined ? null : `updated ${agoLabel(asOf, nowMs)}`,
  ]
    .filter(Boolean)
    .join(" · ");

  const refresh = useCallback(() => void refreshAccounts(), [refreshAccounts]);
  // Re-probe on both endings, as SignInSection does: a user who closes the
  // terminal after finishing in a browser tab gets the same refresh.
  const close = useCallback(() => {
    onCloseSignIn();
    refresh();
  }, [onCloseSignIn, refresh]);

  return (
    <div className={`set-prov-acct ${account.active ? "active" : ""}`}>
      <div className="set-prov-acct-main flex-center">
        <input
          id={inputId}
          type="radio"
          name={`account-${providerId}`}
          checked={account.active}
          onChange={choose}
        />
        <label htmlFor={inputId} className="set-prov-acct-name text-base">
          {accountLabel(account)}
        </label>
        <IconButton
          size="xs"
          tip="Refresh usage"
          disabled={refreshing || signingIn || account.status === "signed_out"}
          onClick={() => void refreshOwn()}
        >
          {refreshing ? <Spinner size={12} /> : <Icon name="refresh" size={12} />}
        </IconButton>
        {account.status !== "signed_in" && (
          <ProviderAuthBadge status={account.status} detail={account.detail} />
        )}
        <span className="set-prov-acct-sub text-xs truncate">{sub}</span>

        {confirming ? (
          <>
            <span className="set-prov-acct-confirm text-sm">{confirming.confirm}</span>
            <Button
              variant="outline"
              size="sm"
              danger={confirming.danger}
              disabled={busy}
              onClick={() => void run(confirming)}
            >
              {confirming.confirmLabel}
            </Button>
            <Button variant="ghost" size="sm" disabled={busy} onClick={() => setConfirming(null)}>
              Cancel
            </Button>
          </>
        ) : (
          <>
            {command && !signingIn && needsSignIn && (
              <Button
                variant={account.status === "signed_out" ? "primary" : "outline"}
                size="sm"
                onClick={onSignIn}
              >
                Sign in
              </Button>
            )}
            <AccountMenu actions={actions} disabled={signingIn} onPick={pick} />
          </>
        )}
      </div>

      <LimitsPanel providerId={providerId} account={account} row={limits} nowMs={nowMs} />

      {/* The probe's reason, in the open: a badge alone sends the user
          chasing a login they may already have under another name. */}
      {account.status === "signed_out" && account.detail && (
        <p className="set-prov-acct-detail text-sm">{account.detail}</p>
      )}
      {error && <p className="set-prov-acct-error text-sm">{error}</p>}

      {signingIn && command && (
        <ProviderLoginTerminal
          providerId={providerId}
          accountId={accountId}
          providerLabel={providerLabel}
          onClose={close}
          onFinished={refresh}
        />
      )}
    </div>
  );
}
