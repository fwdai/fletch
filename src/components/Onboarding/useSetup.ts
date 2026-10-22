// The machine's readiness, as the pre-flight reads it: git, the GitHub
// connection, and the agent CLIs. One instance lives in the Onboarding shell
// so the probes start the moment onboarding mounts (usually while the welcome
// is still up) and every row, panel and event reads the same truth.

import { useCallback, useEffect, useState } from "react";
import { api, type GhStatus, type ToolStatus } from "@/api";
import { PROVIDERS } from "@/data/providers";
import { useAppStore } from "@/store";
import type { GitDistState } from "@/util/useGitDist";
import { useGitInstall } from "@/util/useGitInstall";
import { useProviderPoll } from "@/util/useProviderPoll";

export interface OnboardingSetup {
  /** Result of the git probe; null until the first check resolves (or when
   *  it failed and nothing earlier answered). */
  git: ToolStatus | null;
  /** The git probe has settled — answered or failed — since the last
   *  `recheck`. A row waits on this, not on `git` being non-null: a probe
   *  that fails must still let the sequence move. */
  gitProbed: boolean;
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
  /** The GitHub status probe has settled since the last `recheck`. Until it
   *  has, `ghConnected` is only the last answer, so nothing may act on it. */
  ghProbed: boolean;
  ghConnected: boolean;

  /** Providers the user hasn't toggled off — the onboarding agent set. */
  agents: typeof PROVIDERS;
  detected: number;
  hasAgent: boolean;
  providersProbed: boolean;
  providerVersions: Record<string, string>;
  providerPaths: Record<string, string>;
  refreshProviders: () => Promise<void>;

  /** Re-run every probe — after a sign-in, an install, a download settling. */
  recheck: () => void;
}

/** `pollAgents` re-probes provider binaries every few seconds — enabled while
 *  the pre-flight is on screen, so an install finishing (one-click or in the
 *  user's own terminal) lights its row up without hunting for a re-check
 *  button. */
export function useOnboardingSetup(pollAgents: boolean): OnboardingSetup {
  const providerVersions = useAppStore((s) => s.providerVersions);
  const providerPaths = useAppStore((s) => s.providerPaths);
  const providersProbed = useAppStore((s) => s.providersProbed);
  const refreshProviders = useAppStore((s) => s.refreshProviderVersions);
  const providerFlags = useAppStore((s) => s.providerFlags);

  const [git, setGit] = useState<ToolStatus | null>(null);
  const [gitProbed, setGitProbed] = useState(false);
  const [gh, setGh] = useState<GhStatus | null>(null);
  const [ghProbed, setGhProbed] = useState(false);

  // Each probe lands on its own and marks itself settled whether it answered
  // or failed; a failure keeps the last answer (none, on the first run) rather
  // than failing the rest. The flags drop while a re-check is in flight so a
  // row re-read after a change (a sign-in, an install) reads "checking" until
  // the new answer is in, instead of acting on the old one.
  const recheck = useCallback(() => {
    void refreshProviders();
    setGitProbed(false);
    void api
      .checkCli("git")
      .then(setGit, () => {})
      .finally(() => setGitProbed(true));
    setGhProbed(false);
    void api
      .ghStatus()
      .then(setGh, () => {})
      .finally(() => setGhProbed(true));
  }, [refreshProviders]);

  useEffect(() => {
    recheck();
  }, [recheck]);

  useProviderPoll(pollAgents);

  const { gitDist, gitDownloading, gitInstallError, installingGit, installGit } = useGitInstall(
    git,
    recheck,
  );

  const agents = PROVIDERS.filter((p) => providerFlags[p.id] !== false);
  const detected = agents.filter((p) => !!providerPaths[p.id]).length;

  return {
    git,
    gitProbed,
    gitDist,
    gitReady: !!git?.installed,
    gitDownloading,
    gitInstallError,
    installingGit,
    installGit,
    gh,
    ghProbed,
    ghConnected: !!gh?.authenticated,
    agents,
    detected,
    hasAgent: detected > 0,
    providersProbed,
    providerVersions,
    providerPaths,
    refreshProviders,
    recheck,
  };
}
