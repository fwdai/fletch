import { Toggle } from "@/components/Settings/Toggle";
import { useGate } from "@/store/capabilities";
import { useProjectSettings } from "@/util/useProjectSettings";
import {
  AUTOQUEUE_KEY,
  CONCURRENCY_CHOICES,
  DEFAULT_MAX_CONCURRENT,
  flagOn,
  MAX_CONCURRENT_KEY,
  MIDRUN_AWARENESS_KEY,
  parseCap,
  SETTLE_REVIEW_KEY,
} from "./Roadmap/autonomy";

/** The autonomy dial: how much of the roadmap pipeline runs without you.
 *
 *  Four per-project settings, all read host-side (`roadmap/drainer.rs`,
 *  `roadmap/review.rs`) — the keys and the spellings live in `Roadmap/autonomy.ts`,
 *  which both this section and the board read, so nothing here decides anything
 *  the queue doesn't also see.
 *
 *  Each row follows the neighbouring sections' pattern: read from the host the UI
 *  is driving, write (or delete) on change, absent meaning the default, and
 *  follow `project_settings:changed` from other clients. Deleting rather than writing the
 *  default keeps "never configured" and "configured back to the default" the same
 *  row, which is what makes a future change of default actually reach old
 *  projects. */
export function RoadmapSection({ projectId }: { projectId: string }) {
  // `save(key, null)` deletes the row: "back to the default".
  const { settings, save } = useProjectSettings(projectId);
  const gate = useGate("projectSettings");
  const locked = !!gate || settings === null;
  const all = settings ?? {};
  const autoqueue = flagOn(all[AUTOQUEUE_KEY], false);
  const cap = parseCap(all[MAX_CONCURRENT_KEY]);
  const settleReview = flagOn(all[SETTLE_REVIEW_KEY], true);
  const midrunAwareness = flagOn(all[MIDRUN_AWARENESS_KEY], true);

  const toggleAutoqueue = (next: boolean) => save(AUTOQUEUE_KEY, next ? "1" : null);

  const pickCap = (next: number) =>
    save(MAX_CONCURRENT_KEY, next === DEFAULT_MAX_CONCURRENT ? null : String(next));

  // On is the default for the two PM switches, so *off* is the row that has to
  // exist.
  const toggleSettleReview = (next: boolean) => save(SETTLE_REVIEW_KEY, next ? null : "0");

  const toggleMidrunAwareness = (next: boolean) => save(MIDRUN_AWARENESS_KEY, next ? null : "0");

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Roadmap</h2>
        <p className="ps-section-lead text-sm">
          How much of the roadmap runs without you. Every item still starts as something you accept
          — these decide what happens after that, and a hold (yours or the PM&rsquo;s) keeps an item
          out of the queue no matter how they are set.
        </p>
      </header>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-rm-autoqueue">
          Accepted items queue automatically
        </label>
        <Toggle
          value={autoqueue}
          onChange={toggleAutoqueue}
          disabled={locked}
          title={gate ?? undefined}
        />
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
          disabled={locked}
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
        <Toggle
          value={settleReview}
          onChange={toggleSettleReview}
          disabled={locked}
          title={gate ?? undefined}
        />
      </div>
      <p className="ps-section-lead text-sm">
        When a run settles, the PM agent reads the outcome against the item it wrote and records
        what deviated — as notes on the card and proposals you rule on. On by default; it costs one
        chat turn per finished run, and turning it off leaves nobody watching whether the work
        matched the plan.
      </p>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-rm-midrun">
          The PM follows runs as they happen
        </label>
        <Toggle
          value={midrunAwareness}
          onChange={toggleMidrunAwareness}
          disabled={locked}
          title={gate ?? undefined}
        />
      </div>
      <p className="ps-section-lead text-sm">
        A run reports its progress while it works. With this on the PM reads those as they arrive,
        so it can flag a run building the wrong thing before its pull request exists, and hold the
        item so nothing else builds on it — instead of judging it afterwards. Canceling the run
        itself stays with you. Off, the PM only sees finished runs. Questions a run asks you are
        never routed here either; those stay yours.
      </p>
    </section>
  );
}
