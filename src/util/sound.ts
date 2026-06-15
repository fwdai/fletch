// Best-effort notification sounds. Assets live in public/ and are served at
// the web root, so no bundler import is needed.

let agentDone: HTMLAudioElement | null = null;

/** Play the chime that signals an agent turn finished successfully. Reuses a
 *  single Audio element and rewinds it so back-to-back turn-ends each chime.
 *  All failures (missing file, autoplay policy, no audio device) are swallowed —
 *  a notification sound is never important enough to surface as an error. */
export function playAgentDone(): void {
  try {
    if (!agentDone) {
      agentDone = new Audio("/agent_done.mp3");
      agentDone.volume = 0.5;
    }
    agentDone.currentTime = 0;
    void agentDone.play().catch(() => {});
  } catch {
    // ignore — audio is best-effort
  }
}
