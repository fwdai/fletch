// How loud the microphone is right now, for the composer's waveform. A port of
// `src-tauri/src/dictation/level.rs`: the same range and the same mapping, so
// the bars read the same on the phone as on the Mac.

/** How often a level is reported while the mic is open — the lerp tick of the
 *  bars that display it. */
export const LEVEL_POLL_MS = 90;

/** How many recent levels the phone's full-width waveform shows. */
export const LEVEL_BARS = 12;

/** The dBFS range the bars span: below the floor is silence, the ceiling is
 *  loud, close speech. Linear in dB between, because that is how loudness
 *  reads.
 *
 *  −54 dBFS is where the silence detector's `MIN_RMS` (0.002) sits — the
 *  quietest a buffer can be and still be called speech. At −50 the bars
 *  rendered empty for audio the detector was hearing as speech, so the waveform
 *  contradicted the session's own behaviour: the user watched a flat line while
 *  the mic stayed open on them. Move it with `level.rs`'s. */
export const FLOOR_DB = -54;
export const CEIL_DB = -15;

/** Map a linear RMS onto the bars' 0–1 range. */
export function normalizeLevel(rms: number): number {
  if (rms <= 0) return 0;
  const db = 20 * Math.log10(rms);
  return Math.min(1, Math.max(0, (db - FLOOR_DB) / (CEIL_DB - FLOOR_DB)));
}
