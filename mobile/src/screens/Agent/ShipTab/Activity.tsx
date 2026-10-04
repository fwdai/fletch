import { fmtAgo, useTick } from "../../../lib/hooks";
import type { ShipActivityEntry } from "../../../store/shipActivity";

/** What moved the work along since this session connected — plus what the
 *  host's autopilot did, which it keeps — newest first (store/shipActivity).
 *  Nothing to draw renders nothing. */
export function Activity({ entries }: { entries: ShipActivityEntry[] | undefined }) {
  const any = !!entries?.length;
  // The relative times age on their own; a half-minute tick keeps "just now"
  // honest without re-rendering on every second.
  useTick(30_000, any);
  if (!entries || !any) return null;
  const now = Date.now();
  return (
    <div className="card ship-activity">
      {entries.map((e) => (
        <div key={e.id ?? `${e.at}:${e.text}`} className="ship-act">
          <span className="txt">{e.text}</span>
          <span className="when mono">{fmtAgo(e.at, now)}</span>
        </div>
      ))}
    </div>
  );
}
