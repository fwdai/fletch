import { type RefObject, useState } from "react";
import { ignore } from "../../lib/ignore";
import { useStore } from "../../store";

/** The head of a paged transcript: offered while the host has older pages
 *  than the ones loaded, and gone once the history is exhausted (or on a host
 *  that served it whole). */
export function LoadOlder({
  agentId,
  scroller,
}: {
  agentId: string;
  /** The log's scroll container, held still across the prepend. */
  scroller: RefObject<HTMLDivElement>;
}) {
  const older = useStore((s) => s.histories[agentId]?.older);
  const loadOlderLog = useStore((s) => s.loadOlderLog);
  const [loading, setLoading] = useState(false);
  if (!older) return null;

  const load = async () => {
    const el = scroller.current;
    // Measured from the bottom, which is the edge that does not move: the
    // page lands above what the user is reading, and without this the view
    // would jump by its height. WebKit has no scroll anchoring to do it.
    const fromBottom = el ? el.scrollHeight - el.scrollTop : 0;
    setLoading(true);
    // A failure is already in `lastError`, where the error UI reads it.
    await loadOlderLog(agentId).catch(ignore);
    setLoading(false);
    requestAnimationFrame(() => {
      if (el) el.scrollTop = el.scrollHeight - fromBottom;
    });
  };

  return (
    <button type="button" className="btn ghost sm load-older" disabled={loading} onClick={load}>
      {loading ? "Loading…" : "Load older messages"}
    </button>
  );
}
