import { useEffect } from "react";
import { useAppStore } from "@/store";

/** How often a surface showing install state re-probes the agent binaries. */
export const PROVIDER_PROBE_INTERVAL_MS = 4000;

/** Re-probe the agent CLI binaries every few seconds while `active` — shared by
 *  onboarding's agents step and Settings › Providers, the two surfaces where an
 *  install finishing (ours, or one the user ran in their own terminal) should
 *  light a row up without hunting for a re-scan button. */
export function useProviderPoll(active = true): void {
  const refreshProviders = useAppStore((s) => s.refreshProviderVersions);
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => void refreshProviders(), PROVIDER_PROBE_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [active, refreshProviders]);
}
