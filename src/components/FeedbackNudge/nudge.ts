// When to show the one-time feedback prompt. Pure, so the schedule is testable
// without a store or a DB.
//
// The rule: count the distinct calendar days the app was opened on, and show
// the prompt on the third one. Days need not be consecutive — someone who
// opens Fletch on Monday, then two weeks later, then a month after that gets
// the prompt on that third visit. Once it has been shown it is never shown
// again, whatever they clicked (or didn't).

/** Settings row holding the schedule, as JSON (`Nudge`). */
export const FEEDBACK_NUDGE_KEY = "feedbackNudge";

/** Which distinct day of use the prompt comes up on. */
export const SHOW_ON_DAY = 3;

export interface Nudge {
  /** Local calendar days the app was opened on, `YYYY-MM-DD`, oldest first. */
  days: string[];
  /** The prompt has been shown; nothing more to do, ever. */
  shown: boolean;
}

/** Today as a local `YYYY-MM-DD` — local, because "the next day" to the user
 *  is their calendar, not UTC's. */
export function localDay(now: Date = new Date()): string {
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  const d = String(now.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

/** The stored schedule, or a blank one when the row is absent or unreadable. */
export function parseNudge(raw: string | null): Nudge {
  if (!raw) return { days: [], shown: false };
  try {
    const v = JSON.parse(raw) as Partial<Nudge>;
    return {
      days: Array.isArray(v.days) ? v.days.filter((d) => typeof d === "string") : [],
      shown: v.shown === true,
    };
  } catch {
    return { days: [], shown: false };
  }
}

/** Record today's visit and decide. `show` is true on exactly one call ever:
 *  the first one on the `SHOW_ON_DAY`th distinct day. The returned `next` is
 *  what to persist; it is the input, untouched, when nothing changed. */
export function visit(prev: Nudge, today: string): { next: Nudge; show: boolean } {
  if (prev.shown) return { next: prev, show: false };
  const days = prev.days.includes(today) ? prev.days : [...prev.days, today];
  const show = days.length >= SHOW_ON_DAY;
  if (days === prev.days && !show) return { next: prev, show: false };
  return { next: { days, shown: show }, show };
}
