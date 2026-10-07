// The accounts half of a provider that can hold several sign-ins (claude,
// codex — `supportsAccounts`): every account Fletch knows for it, which one
// new agents use, a Sign in per account, and a way to add one. Takes the
// place of SignInSection for those providers; the single-login providers
// keep SignInSection.
//
// One sign-in terminal at a time, like SignInSection: the section tracks
// which account's is open, so two logins can't race each other for the
// browser.

import { useState } from "react";
import type { ProviderId } from "@/data/providers";
import { useAppStore } from "@/store";
import { AccountRow } from "./AccountRow";
import { AddAccountForm } from "./AddAccountForm";

export function AccountsSection({
  providerId,
  providerLabel,
}: {
  providerId: ProviderId;
  providerLabel: string;
}) {
  const accounts = useAppStore((s) => s.providerAccounts[providerId]);
  const [signingIn, setSigningIn] = useState<string | null>(null);

  // Not listed yet (first probe in flight): nothing to show rather than an
  // empty section that then jumps.
  if (!accounts) return null;

  return (
    <div className="set-prov-accounts">
      <div className="set-prov-acct-head text-sm">
        <span className="set-prov-dk">Accounts</span>
        <span>
          New agents use the selected account. Each account keeps its own login and sessions;
          settings, commands and skills are shared from your terminal setup.
        </span>
      </div>
      {accounts.map((account) => (
        <AccountRow
          key={account.id}
          account={account}
          providerId={providerId}
          providerLabel={providerLabel}
          signingIn={signingIn === account.id}
          onSignIn={() => setSigningIn(account.id)}
          onCloseSignIn={() => setSigningIn(null)}
        />
      ))}
      <AddAccountForm providerId={providerId} />
    </div>
  );
}
