// The sign-in half of an installed provider's detail: an offer to run the
// CLI's own login, and the embedded terminal that runs it.
//
// Detecting a binary is not detecting an account, so this is the one place in
// the row that acts on `providerAuth`. It shows one thing at a time — the
// offer, or the terminal (which carries its own Close and hint) — so the
// detail never grows two competing sign-in controls.

import { useCallback, useState } from "react";
import { Button } from "@/components/ui/Button";
import { DocsLink } from "@/components/ui/DocsLink";
import { loginCommand, PROVIDER_DETAIL } from "@/data/providerDetail";
import type { ProviderId } from "@/data/providers";
import { useAppStore } from "@/store";
import { ProviderLoginTerminal } from "../ProviderLogin";

export function SignInSection({
  providerId,
  providerLabel,
}: {
  providerId: ProviderId;
  providerLabel: string;
}) {
  const status = useAppStore((s) => s.providerAuth[providerId]);
  // Stable across renders (a zustand action), so the terminal's one-shot
  // outcome effect isn't re-armed by a parent re-render.
  const refreshProviderAuth = useAppStore((s) => s.refreshProviderAuth);
  const [open, setOpen] = useState(false);

  const d = PROVIDER_DETAIL[providerId];
  const command = loginCommand(providerId);

  // Re-probe on both endings: a clean exit is the usual one, but a user who
  // closes the terminal after completing auth in a browser tab gets the same
  // refresh rather than having to hit Re-scan.
  const refresh = useCallback(() => void refreshProviderAuth(), [refreshProviderAuth]);
  const close = useCallback(() => {
    setOpen(false);
    refresh();
  }, [refresh]);

  if (open && command) {
    return (
      <ProviderLoginTerminal
        providerId={providerId}
        providerLabel={providerLabel}
        onClose={close}
        onFinished={refresh}
      />
    );
  }

  // No login command (antigravity, pi): there is nothing to run in-app, so the
  // row offers the hint — or, lacking one, the docs, mirroring how the missing
  // state falls back from Install to an install guide.
  if (!command) {
    return (
      <div className="set-prov-detail-actions flex-center">
        {d.signIn ? (
          <span className="prov-login-hint text-sm">{d.signIn}</span>
        ) : (
          <DocsLink url={d.docs} label="Sign-in guide" />
        )}
      </div>
    );
  }

  return (
    <div className="set-prov-detail-actions flex-center">
      {/* A signed-out CLI is installed but unusable, so its sign-in is the
          row's primary action; an already-signed-in one keeps it quiet. */}
      <Button
        variant={status === "signed_out" ? "primary" : "outline"}
        size="sm"
        onClick={() => setOpen(true)}
      >
        Sign in
      </Button>
      {d.signIn && <span className="prov-login-hint text-sm">{d.signIn}</span>}
    </div>
  );
}
