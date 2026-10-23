// When to show the one-time feedback prompt. Pure, so the schedule is testable
// without a store, a DB or a DOM.
//
// The rule: count the distinct calendar days the app was opened on, and show
// the prompt on the third one. Days need not be consecutive — someone who
// opens Fletch on Monday, then two weeks later, then a month after that gets
// the prompt on that third visit. Once it has been shown it is never shown
// again, whatever they clicked (or didn't).
//
// Two steps, because they wait on different things. Recording today's visit
// waits only for the settings to be hydrated. Showing waits for onboarding to
// be closed as well: the store's defaults say onboarding is closed until
// hydration says otherwise, and a user who left the tour unfinished must not
// get the prompt raised under it — nor silently spent, since it shows once.

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

/** Today's visit, recorded. The input itself when today is already there. */
export function recordDay(prev: Nudge, today: string): Nudge {
  if (prev.days.includes(today)) return prev;
  return { ...prev, days: [...prev.days, today] };
}

/** Is the prompt owed right now: enough days, and not yet shown. */
export function isDue(n: Nudge): boolean {
  return !n.shown && n.days.length >= SHOW_ON_DAY;
}

/** What the app looks like to the scheduler. */
export interface SchedulerState {
  /** `hydrateSettings` has run (see `AppSlice.settingsHydrated`). */
  hydrated: boolean;
  onboardingOpen: boolean;
}

export interface SchedulerDeps {
  load: () => Promise<string | null>;
  save: (n: Nudge) => Promise<void>;
  /** Raise the prompt. */
  open: () => void;
  today: () => string;
}

/** The schedule as a function of app state. Call `tick` whenever `hydrated` or
 *  `onboardingOpen` changes; it records the day on the first hydrated tick and
 *  opens the prompt — marking it shown first, so a quit mid-prompt still counts
 *  — on the first tick that is both hydrated and clear of onboarding while the
 *  prompt is due. Ticks are serialized, so a fast flip of `onboardingOpen` while
 *  the DB is still answering cannot act twice on stale state. */
export function createScheduler(deps: SchedulerDeps): (state: SchedulerState) => Promise<void> {
  let nudge: Nudge | null = null;
  let finished = false;
  let chain = Promise.resolve();

  const step = async (state: SchedulerState) => {
    if (finished || !state.hydrated) return;
    if (nudge === null) {
      const recorded = recordDay(parseNudge(await deps.load()), deps.today());
      if (recorded.days.length > 0) await deps.save(recorded);
      nudge = recorded;
    }
    if (!isDue(nudge)) {
      // Not this session: either too early (then the day is on record and the
      // next session re-counts) or already shown (then never again).
      finished = true;
      return;
    }
    if (state.onboardingOpen) return;
    finished = true;
    nudge = { ...nudge, shown: true };
    await deps.save(nudge);
    deps.open();
  };

  return (state) => {
    chain = chain.then(() => step(state)).catch(() => {});
    return chain;
  };
}
