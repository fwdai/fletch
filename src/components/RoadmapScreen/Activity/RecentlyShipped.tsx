import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useEffect, useState } from "react";
import { Icon } from "@/components/Icon";
import { Skeleton } from "@/components/Stats";
import { Button } from "@/components/ui/Button";
import { formatAge } from "@/util/format";
import { loadRecentlyShipped, type ShippedItem } from "./activityData";

const LIMIT = 10;

/** The last few roadmap items this project shipped, newest first.
 *
 *  The counterpart to the Roadmap tab: done items leave the board entirely, so
 *  without this the only trace of them is a number in the page header. */
export function RecentlyShipped({ projectId }: { projectId: string }) {
  const [items, setItems] = useState<ShippedItem[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    setItems(null);
    loadRecentlyShipped(projectId, LIMIT)
      .then((rows) => !cancelled && setItems(rows))
      .catch((err) => console.error("recently shipped failed", err));
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  const now = Date.now();

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Recently shipped</h2>
        <p className="ps-section-lead text-sm">
          The last {LIMIT} roadmap items marked done. They’re off the board now, so this is where
          they stay visible.
        </p>
      </header>

      {items == null ? (
        <Skeleton height={132} />
      ) : items.length === 0 ? (
        <div className="mb-empty text-sm">
          Nothing shipped yet. Items land here as the board marks them done.
        </div>
      ) : (
        <ul className="act-ship">
          {items.map((it) => (
            <li key={it.id} className="act-ship-row">
              <span className="act-ship-code mono text-xs">{it.code}</span>
              <span className="act-ship-title text-sm">{it.title}</span>
              <span className="act-ship-age text-xs">{formatAge(it.updated_at, now)}</span>
              {it.pr_url && (
                <Button
                  variant="outline"
                  size="sm"
                  className="act-ship-pr"
                  onClick={() => {
                    void openExternal(it.pr_url as string).catch(() => {});
                  }}
                >
                  <Icon name="pr" size={11} />
                  {it.pr_number ? `#${it.pr_number}` : "PR"}
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
