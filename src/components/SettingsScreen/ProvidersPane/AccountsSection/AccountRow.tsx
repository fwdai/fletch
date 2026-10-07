import { useCallback, useState } from "react";
import type { AccountLimits, ProviderAccount } from "@/api/types/providers";
import { ProviderAuthBadge, ReauthButton } from "@/components/SettingsScreen/ProviderAuthBadge";
import { ProviderLoginTerminal } from "@/components/SettingsScreen/ProviderLogin";
import { Button } from "@/components/ui/Button";
import { accountLabel } from "@/data/providerAccounts";
import { loginCommand } from "@/data/providerDetail";
import type { ProviderId } from "@/data/providers";
import { useAppStore } from "@/store";
import { LimitsPanel } from "./LimitsPanel";

/** One account: the radio that makes it the one new agents use, its name and
 *  sign-in state, and its actions. Sign in opens the same embedded terminal
 *  as a single-login provider, run against this account's directory. Remove
 *  asks once inline — it deletes the account's login and transcripts — and is
 *  never offered for the default (the CLI's own directory) or the active one
 *  (the backend refuses that too; pick another first). Under it, the
 *  account's plan limits. */
export function AccountRow({
  account,
  providerId,
  providerLabel,
  signingIn,
  onSignIn,
  onCloseSignIn,
  limits,
  nowMs,
}: {
  account: ProviderAccount;
  providerId: ProviderId;
  providerLabel: string;
  signingIn: boolean;
  limits: AccountLimits | undefined;
  nowMs: number;
  onSignIn: () => void;
  onCloseSignIn: () => void;
}) {
  const setActive = useAppStore((s) => s.setActiveProviderAccount);
  const remove = useAppStore((s) => s.removeProviderAccount);
  // Stable (a zustand action), so the terminal's one-shot outcome effect
  // isn't re-armed by a parent re-render — same contract as SignInSection.
  const refreshAccounts = useAppStore((s) => s.refreshProviderAccounts);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const command = loginCommand(providerId);
  const inputId = `account-${providerId}-${account.id}`;
  const accountId = account.managed ? account.id : undefined;
  // Offered unless the account is known to be signed in — or a limits refresh
  // just found it signed out, whose hint below points at this button.
  const needsSignIn = account.status !== "signed_in" || limits?.refresh?.status === "signed_out";

  const choose = () => {
    setError(null);
    setActive(providerId, accountId ?? null).catch((err) => setError(String(err)));
  };
  const removeNow = () => {
    setError(null);
    remove(providerId, account.id).catch((err) => {
      setError(String(err));
      setConfirming(false);
    });
  };

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
        <ProviderAuthBadge status={account.status} detail={account.detail} />
        {command && !signingIn && !needsSignIn && <ReauthButton onClick={onSignIn} />}
        <span className="set-prov-acct-sub mono text-xs truncate">
          {account.managed
            ? `~/.fletch/accounts/${providerId}/${account.id}`
            : "Your CLI's own login, shared with the terminal"}
        </span>

        {confirming ? (
          <>
            <span className="set-prov-acct-confirm text-sm">
              Delete this account's login and sessions?
            </span>
            <Button variant="outline" size="sm" danger onClick={removeNow}>
              Delete
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setConfirming(false)}>
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
            {account.managed && !account.active && (
              <Button variant="ghost" size="sm" danger onClick={() => setConfirming(true)}>
                Remove
              </Button>
            )}
          </>
        )}
      </div>

      <LimitsPanel providerId={providerId} account={account} row={limits} nowMs={nowMs} />

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
