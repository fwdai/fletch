// The open agent's git state, kept current while its screen is up. The rule is
// the desktop's (src/store/gitSync.ts): the host pushes GitHub facts on change,
// so the PR half follows the `pr:*` events, and only the local git read the
// screen is rendering runs on a timer.

import { usePoll } from "../../lib/hooks";
import { agentOf, useStore } from "../../store";

/** Brisk while Changes or Ship shows the diff, slow behind the chat, where only
 *  the header line uses it. Gentler than the desktop's 1 s: the phone reads
 *  over the LAN or the relay. */
export const gitFollowMs = (pane: string) => (pane === "chat" ? 10_000 : 3_000);

/** Poll `loadGitState` for `agentId` while connected. A planning chat has no
 *  checkout to read (see `loadAgent`), so it polls nothing. `usePoll` reads
 *  at once whenever the interval changes, so switching to Changes or Ship
 *  shows a fresh tree rather than waiting out the chat's interval. */
export function useGitFollow(agentId: string, pane: string) {
  const loadGitState = useStore((s) => s.loadGitState);
  const active = useStore((s) => {
    const agent = agentOf(s, agentId);
    return s.connection === "connected" && agent !== undefined && !agent.purpose;
  });
  usePoll(() => void loadGitState(agentId), gitFollowMs(pane), active);
}
