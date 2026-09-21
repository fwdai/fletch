// Which providers the paired host can actually run.
//
// An agent runs on the host, not on the phone, so the static `PROVIDERS` list
// the picker draws from says only which agents Fletch knows about — not which
// of them this host has installed and signed in. `host_providers` answers that
// (docs/remote-protocol.md, "Which providers a host can run").
//
// Advisory in every direction: a host too old for the op is never asked, a
// failed call leaves the picker offering everything, and an `unknown` login
// probe is not a claim. The phone cannot fix either problem — installing and
// signing in are the operator's, on the host — so all it does is say so.

import { useEffect, useState } from "react";
import type { HostProvider } from "../remote";
import { api, useStore } from "../store";

/** The host's provider rows while `active`, or null when the host has not been
 *  asked, cannot be asked, or did not answer. Re-read whenever `active` goes
 *  true, so a sign-in done on the host between two openings of the sheet shows
 *  up on the second. */
export function useHostProviders(active: boolean): HostProvider[] | null {
  const supported = useStore((s) => s.hostSupports("host_providers"));
  const [rows, setRows] = useState<HostProvider[] | null>(null);

  useEffect(() => {
    if (!active || !supported) return;
    let live = true;
    void api
      .hostProviders()
      .then((next) => {
        if (live) setRows(next);
      })
      .catch(() => {
        // Advisory, like the branch list beside it: the picker stays as it was.
      });
    return () => {
      live = false;
    };
  }, [active, supported]);

  return supported ? rows : null;
}

/** Why `providerId` cannot be spawned on the host, in the few words a chip can
 *  carry — or null when it can, and whenever the phone does not know (no rows,
 *  a provider the host did not list, an `unknown` login probe). */
export function hostProviderBlock(rows: HostProvider[] | null, providerId: string): string | null {
  const row = rows?.find((p) => p.id === providerId);
  if (!row) return null;
  if (!row.installed) return "not installed";
  return row.auth === "signed_out" ? "not signed in" : null;
}
