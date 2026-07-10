// Shared readiness state for the functional onboarding steps. One instance
// lives in the Onboarding shell so the git / GitHub / agents steps and the
// footer's gated Continue all read the same truth — a step flipping ready
// enables Continue without re-probing.

import { useCallback, useEffect, useState } from "react";
import { api, type GhStatus, type ToolStatus } from "@/api";
import { PROVIDERS } from "@/data/providers";
import { useAppStore } from "@/store";
import type { GitDistState } from "@/util/useGitDist";
import { useGitInstall } from "@/util/useGitInstall";

export interface OnboardingSetup {
  /** Result of the git probe; null until the first check resolves. */
  git: ToolStatus | null;
  /** Live portable-git bootstrap state (download progress / failure). */
  gitDist: GitDistState;
  gitReady: boolean;
  /** Portable git download in flight (startup bootstrap or manual retry). */
  gitDownloading: boolean;
  /** Why the portable install failed, when it did. */
  gitInstallError?: string;
  installingGit: boolean;
  installGit: () => void;

  gh: GhStatus | null;
  ghConnected: boolean;

  /** Providers the user hasn't toggled off — the onboarding agent set. */
  agents: typeof PROVIDERS;
  detected: number;
  hasAgent: boolean;
  providersProbed: boolean;
  providerVersions: Record<string, string>;
  providerPaths: Record<string, string>;
  refreshProviders: () => Promise<void>;

  checking: boolean;
  recheck: () => void;
}

/** `pollAgents` re-probes provider binaries every few seconds — enabled while
 *  the agents step is on screen, so an install finishing (one-click or in the
 *  user's own terminal) lights its tile up without hunting for a re-check
 *  button. */
export function useOnboardingSetup(pollAgents: boolean): OnboardingSetup {
  const providerVersions = useAppStore((s) => s.providerVersions);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const providersProbed = useAppStore((s) => s.providersProbed);
  const refreshProviders = useAppStore((s) => s.refreshProviderVersions);
  const providerFlags = useAppStore((s) => s.providerFlags);

  const [git, setGit] = useState<ToolStatus | null>(null);
  const [gh, setGh] = useState<GhStatus | null>(null);
  const [checking, setChecking] = useState(true);

  const recheck = useCallback(() => {
    setChecking(true);
    void Promise.allSettled([
      refreshProviders(),
      api.checkCli("git").then(setGit),
      api.ghStatus().then(setGh),
    ]).finally(() => setChecking(false));
  }, [refreshProviders]);

  useEffect(() => {
    recheck();
  }, [recheck]);

  useEffect(() => {
    if (!pollAgents) return;
    const t = window.setInterval(() => void refreshProviders(), 4000);
    return () => window.clearInterval(t);
  }, [pollAgents, refreshProviders]);

  const { gitDist, gitDownloading, gitInstallError, installingGit, installGit } = useGitInstall(
    git,
    recheck,
  );

  const agents = PROVIDERS.filter((p) => providerFlags[p.id] !== false);
  const detected = agents.filter((p) => !!providerPaths[p.id]).length;

  return {
    git,
    gitDist,
    gitReady: !!git?.installed,
    gitDownloading,
    gitInstallError,
    installingGit,
    installGit,
    gh,
    ghConnected: !!gh?.authenticated,
    agents,
    detected,
    hasAgent: detected > 0,
    providersProbed,
    providerVersions,
    providerPaths,
    refreshProviders,
    checking,
    recheck,
  };
}
