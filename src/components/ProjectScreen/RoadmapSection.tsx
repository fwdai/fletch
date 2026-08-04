import { useEffect, useState } from "react";
import { Toggle } from "@/components/Settings/Toggle";
import {
  deleteProjectSetting,
  getProjectSettings,
  setProjectSetting,
} from "@/storage/projectSettings";
import {
  AUTOQUEUE_KEY,
  CONCURRENCY_CHOICES,
  DEFAULT_MAX_CONCURRENT,
  flagOn,
  MAX_CONCURRENT_KEY,
  parseCap,
  SETTLE_REVIEW_KEY,
} from "./Roadmap/autonomy";

/** The autonomy dial: how much of the roadmap pipeline runs without you.
 *
 *  Three per-project settings, all read host-side (`roadmap/drainer.rs`,
 *  `roadmap/review.rs`) — the keys and the spellings live in `Roadmap/autonomy.ts`,
 *  which both this section and the board read, so nothing here decides anything
 *  the queue doesn't also see.
 *
 *  Each row follows the neighbouring sections' pattern: load on mount, write (or
 *  delete) on change, absent meaning the default. Deleting rather than writing the
 *  default keeps "never configured" and "configured back to the default" the same
 *  row, which is what makes a future change of default actually reach old
 *  projects. */
export function RoadmapSection({ projectId }: { projectId: string }) {
  const [autoqueue, setAutoqueue] = useState(false);
  const [cap, setCap] = useState(DEFAULT_MAX_CONCURRENT);
  const [settleReview, setSettleReview] = useState(true);

  useEffect(() => {
    let cancelled = false;
    getProjectSettings(projectId)
      .then((all) => {
        if (cancelled) return;
        setAutoqueue(flagOn(all[AUTOQUEUE_KEY], false));
        setCap(parseCap(all[MAX_CONCURRENT_KEY]));
        setSettleReview(flagOn(all[SETTLE_REVIEW_KEY], true));
      })
      .catch((e) => console.error("load roadmap settings failed", e));
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  /** Persist one dial, treating `value === null` as "back to the default". */
  const save = (key: string, value: string | null) => {
    const write =
      value === null
        ? deleteProjectSetting(projectId, key)
        : setProjectSetting(projectId, key, value);
    write.catch((e) => console.error(`save ${key} failed`, e));
  };

  const toggleAutoqueue = (next: boolean) => {
    setAutoqueue(next);
    save(AUTOQUEUE_KEY, next ? "1" : null);
  };

  const pickCap = (next: number) => {
    setCap(next);
    save(MAX_CONCURRENT_KEY, next === DEFAULT_MAX_CONCURRENT ? null : String(next));
  };

  const toggleSettleReview = (next: boolean) => {
    setSettleReview(next);
    // On is the default here, so *off* is the row that has to exist.
    save(SETTLE_REVIEW_KEY, next ? null : "0");
  };

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Roadmap</h2>
        <p className="ps-section-lead text-sm">
          How much of the roadmap runs without you. Every item still starts as something you accept
          — these decide what happens after that, and a hold (yours or the PM&rsquo;s) overrules all
          three.
        </p>
      </header>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-rm-autoqueue">
          Accepted items queue automatically
        </label>
        <Toggle value={autoqueue} onChange={toggleAutoqueue} />
      </div>
      <p className="ps-section-lead text-sm">
        With this on, accepting a proposal is the only touch before a pull request arrives —
        &ldquo;Accept&rdquo; hands the item straight to the queue. Off, accepting puts it on the
        roadmap and you queue it when you&rsquo;re ready (the card still offers &ldquo;Accept &amp;
        queue&rdquo; for one-click cases).
      </p>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-rm-concurrency">
          Runs at once
        </label>
        <select
          id="ps-rm-concurrency"
          className="ps-input text-base"
          value={String(cap)}
          onChange={(e) => pickCap(Number(e.target.value))}
        >
          {CONCURRENCY_CHOICES.map((n) => (
            <option key={n} value={n}>
              {n === DEFAULT_MAX_CONCURRENT ? `${n} — one at a time` : n}
            </option>
          ))}
        </select>
      </div>
      <p className="ps-section-lead text-sm">
        More than one queued item builds at a time, each in its own run. They land parallel pull
        requests into the same repo, so past two or three they start conflicting with each other —
        and every one of them still needs your review.
      </p>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-rm-settle-review">
          The PM reviews every finished run
        </label>
        <Toggle value={settleReview} onChange={toggleSettleReview} />
      </div>
      <p className="ps-section-lead text-sm">
        When a run settles, the PM agent reads the outcome against the item it wrote and records
        what deviated — as notes on the card and proposals you rule on. On by default; turning it
        off costs one chat turn per finished run and leaves nobody watching whether the work matched
        the plan.
      </p>
    </section>
  );
}
