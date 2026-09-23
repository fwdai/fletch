// Best-effort notification sounds. Assets live in public/ and are served at
// the web root, so no bundler import is needed.

/** One sound per kind of out-of-app event, so each is recognisable by ear. */
const SOUNDS = {
  success: "/success.mp3",
  alert: "/alert.mp3",
  error: "/error.mp3",
} as const;

export type SoundKind = keyof typeof SOUNDS;

const cache: Partial<Record<SoundKind, HTMLAudioElement>> = {};

/** Play the sound for `kind`. Reuses one Audio element per kind and rewinds it
 *  so back-to-back events each play. All failures (missing file, autoplay
 *  policy, no audio device) are swallowed — a notification sound is never
 *  important enough to surface as an error. */
export function playSound(kind: SoundKind): void {
  try {
    let audio = cache[kind];
    if (!audio) {
      audio = new Audio(SOUNDS[kind]);
      audio.volume = 0.5;
      cache[kind] = audio;
    }
    audio.currentTime = 0;
    void audio.play().catch(() => {});
  } catch {
    // ignore — audio is best-effort
  }
}
