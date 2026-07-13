//! The run budget ledger and enforcement (spec §11.1–11.2).
//!
//! Turns are the universal unit — every provider is turn- and wall-clock-bounded.
//! Tokens are enforced only where the provider's session records expose usage
//! (§11.2 guardrail): the driver sums `usage` out of ingested record bodies and
//! this ledger charges the per-turn delta. A provider that emits no usage reports
//! `None`, its token count never grows, and only turn/clock budgets bite.
//!
//! Two concerns live here, kept apart:
//!   * [`EffectiveBudgets`] — the launch-frozen caps (`§11.1 defaults ∪ spec`,
//!     spec wins). Serialized into `wf_run.budgets_json`; `wf_resume`'s budget
//!     patch bumps the three run-level caps in place.
//!   * [`Ledger`] — the running spend (`wf_run.spent_json`). Its run-level totals
//!     drive enforcement; per-step / per-attempt rollups are for the timeline.
//!
//! Enforcement is a pure predicate ([`Ledger::exceeded`]); the scheduler and the
//! attempt lifecycle call it at the three §11.2 points (before spawn, before each
//! prompt send, at each turn end) and pause the run on a hit.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::driver::TurnUsage;
use super::spec::{Budgets, Spec};

// ───────────────────────────── §11.1 defaults ───────────────────────────────

/// Run-level turn cap when the spec sets none (§11.1).
const DEFAULT_TURNS: i64 = 100;
/// Run-level wall-clock cap, minutes (§11.1).
const DEFAULT_WALL_CLOCK_MINS: i64 = 480;
/// Per-step turn cap (§11.1).
const DEFAULT_TURNS_PER_ATTEMPT: i64 = 10;
/// Per-step retry cap (§11.1) — autonomous retries only.
const DEFAULT_MAX_ATTEMPTS: i64 = 2;
const DEFAULT_SPAWN_TIMEOUT_SECS: i64 = 180;
const DEFAULT_TURN_START_TIMEOUT_SECS: i64 = 120;
const DEFAULT_STALL_TIMEOUT_SECS: i64 = 600;
const DEFAULT_NUDGE_TIMEOUT_SECS: i64 = 300;
const DEFAULT_TESTS_TIMEOUT_SECS: i64 = 900;

/// The launch-frozen budget set: every §11.1 field resolved to a concrete value
/// (tokens stays optional — unlimited unless opted in). Produced once at launch
/// by merging the spec's run-level budgets over the defaults, then persisted to
/// `budgets_json` as the immutable-except-by-patch source of truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectiveBudgets {
    // Run-scoped — the three caps that pause the run when exceeded.
    pub turns: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    pub wall_clock_mins: i64,
    // Step-scoped — per-attempt caps and the attempt timeouts. A step's own
    // `budgets` override these (resolved at attempt time via [`Self::for_step`]).
    pub turns_per_attempt: i64,
    pub max_attempts: i64,
    pub spawn_timeout_secs: i64,
    pub turn_start_timeout_secs: i64,
    pub stall_timeout_secs: i64,
    pub nudge_timeout_secs: i64,
    pub tests_timeout_secs: i64,
}

impl Default for EffectiveBudgets {
    fn default() -> Self {
        Self {
            turns: DEFAULT_TURNS,
            tokens: None,
            wall_clock_mins: DEFAULT_WALL_CLOCK_MINS,
            turns_per_attempt: DEFAULT_TURNS_PER_ATTEMPT,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            spawn_timeout_secs: DEFAULT_SPAWN_TIMEOUT_SECS,
            turn_start_timeout_secs: DEFAULT_TURN_START_TIMEOUT_SECS,
            stall_timeout_secs: DEFAULT_STALL_TIMEOUT_SECS,
            nudge_timeout_secs: DEFAULT_NUDGE_TIMEOUT_SECS,
            tests_timeout_secs: DEFAULT_TESTS_TIMEOUT_SECS,
        }
    }
}

impl EffectiveBudgets {
    /// `§11.1 defaults ∪ spec.budgets`, spec winning on any set field. Frozen at
    /// launch (spec §11.1). A `tokens` of `None` in the spec leaves tokens
    /// unlimited; any other field falls back to its default.
    pub fn resolve(spec: &Spec) -> Self {
        let mut eff = Self::default();
        if let Some(b) = &spec.budgets {
            eff.overlay(b);
        }
        eff
    }

    /// The effective budgets for one step: the run-level frozen set with the
    /// step's own `budgets` overlaid (spec §11.1: "step values override run
    /// values"). **Only the step/attempt-scoped fields** (`turns_per_attempt`,
    /// `max_attempts`, the timeouts) are overridable here. The run-scoped caps
    /// (`turns` / `tokens` / `wall_clock_mins`) stay frozen at the run level —
    /// `exceeded` reads them run-wide, so a step must not be able to widen or
    /// tighten the run's budget for itself.
    pub fn for_step(&self, step_budgets: Option<&Budgets>) -> Self {
        let mut eff = self.clone();
        if let Some(b) = step_budgets {
            if let Some(v) = b.turns_per_attempt {
                eff.turns_per_attempt = v;
            }
            if let Some(v) = b.max_attempts {
                eff.max_attempts = v;
            }
            if let Some(v) = b.spawn_timeout_secs {
                eff.spawn_timeout_secs = v;
            }
            if let Some(v) = b.turn_start_timeout_secs {
                eff.turn_start_timeout_secs = v;
            }
            if let Some(v) = b.stall_timeout_secs {
                eff.stall_timeout_secs = v;
            }
            if let Some(v) = b.nudge_timeout_secs {
                eff.nudge_timeout_secs = v;
            }
            if let Some(v) = b.tests_timeout_secs {
                eff.tests_timeout_secs = v;
            }
        }
        eff
    }

    /// Overlay any set field of `b` onto `self` — used only for the run-level
    /// resolve, where the spec's run budgets may set both the run caps and the
    /// step-scoped defaults. Positivity is already enforced by `spec::validate`,
    /// so values are trusted here.
    fn overlay(&mut self, b: &Budgets) {
        if let Some(v) = b.turns {
            self.turns = v;
        }
        if b.tokens.is_some() {
            self.tokens = b.tokens;
        }
        if let Some(v) = b.wall_clock_mins {
            self.wall_clock_mins = v;
        }
        if let Some(v) = b.turns_per_attempt {
            self.turns_per_attempt = v;
        }
        if let Some(v) = b.max_attempts {
            self.max_attempts = v;
        }
        if let Some(v) = b.spawn_timeout_secs {
            self.spawn_timeout_secs = v;
        }
        if let Some(v) = b.turn_start_timeout_secs {
            self.turn_start_timeout_secs = v;
        }
        if let Some(v) = b.stall_timeout_secs {
            self.stall_timeout_secs = v;
        }
        if let Some(v) = b.nudge_timeout_secs {
            self.nudge_timeout_secs = v;
        }
        if let Some(v) = b.tests_timeout_secs {
            self.tests_timeout_secs = v;
        }
    }

    /// Apply a resume-time budget patch (§13): add the patch's set run-level
    /// caps to the current ones ("resume with +N turns / +N tokens / +N minutes").
    /// Only the three run-scoped caps are patchable; other fields are ignored.
    /// Additive so the UI's "+N" reads literally; a patched `tokens` lifts an
    /// unlimited budget only if it was already limited (adding to "unlimited" is
    /// a no-op, which is the intended semantics).
    pub fn apply_patch(&mut self, patch: &Budgets) {
        if let Some(v) = patch.turns {
            self.turns += v;
        }
        if let (Some(cur), Some(v)) = (self.tokens, patch.tokens) {
            self.tokens = Some(cur + v);
        }
        if let Some(v) = patch.wall_clock_mins {
            self.wall_clock_mins += v;
        }
    }
}

// ───────────────────────────── which limit ──────────────────────────────────

/// Which run-level cap a check tripped — the `which` in `budget_exceeded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetLimit {
    Turns,
    Tokens,
    WallClock,
}

impl BudgetLimit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Turns => "turns",
            Self::Tokens => "tokens",
            Self::WallClock => "wall_clock",
        }
    }
}

// ───────────────────────────── the ledger ───────────────────────────────────

/// Per-step spend, for the timeline rollup (§16 "attempt/step/run rollup").
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepSpend {
    pub turns: i64,
    #[serde(default)]
    pub tokens: i64,
}

/// The running spend of one run (`wf_run.spent_json`). Run-level totals
/// (`turns`, `tokens`, `wall_ms`) drive enforcement; the per-step / per-attempt
/// maps are rollups for the Run Monitor.
///
/// Wall-clock is stored as *accumulated active driving time* (`wall_ms`) rather
/// than a launch timestamp so a run paused for days doesn't blow its clock the
/// instant it resumes: each drive stamps a transient start with [`Self::start_drive`]
/// and folds the elapsed time back in with [`Self::checkpoint_wall`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    pub turns: i64,
    #[serde(default)]
    pub tokens: i64,
    /// Accumulated active wall-clock across all drives, milliseconds.
    #[serde(default)]
    pub wall_ms: i64,
    #[serde(default)]
    pub steps: HashMap<String, StepSpend>,
    #[serde(default)]
    pub attempts: HashMap<String, i64>,

    /// Turns/tokens reserved by live composed sub-runs (spec §10.3): the parent
    /// carves a budget slice out for each sub-run, so that capacity is unavailable
    /// to the parent's own work while the sub-run runs. `exceeded` counts reserved
    /// against the caps; when a sub-run finishes, the parent releases its
    /// reservation and folds the sub-run's *actual* spend in — never both, so a
    /// slice is never double-counted.
    #[serde(default)]
    pub reserved_turns: i64,
    #[serde(default)]
    pub reserved_tokens: i64,

    /// Transient (never serialized): the current drive's start, set by
    /// [`Self::start_drive`]. `wall_ms` already holds prior drives' time.
    #[serde(skip)]
    drive_start_ms: Option<i64>,
    /// Transient (never serialized): last cumulative token total seen per agent,
    /// so token charges are deltas. Fresh agents start at 0; resume abandons
    /// non-terminal attempts and re-spawns, so no double-count survives a restart.
    #[serde(skip)]
    agent_tokens_seen: HashMap<String, u64>,
}

impl Ledger {
    /// Load the ledger from a persisted `spent_json` value; a missing / malformed
    /// snapshot starts an empty ledger (a fresh run persists `{}`).
    pub fn from_json(spent: &Value) -> Self {
        serde_json::from_value(spent.clone()).unwrap_or_default()
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }

    /// Begin a drive: stamp the wall-clock start (folded back in on checkpoint).
    pub fn start_drive(&mut self, now_ms: i64) {
        self.drive_start_ms = Some(now_ms);
    }

    /// Fold this drive's elapsed active time into `wall_ms` and restamp the start,
    /// so repeated checkpoints within one drive don't double-count. Called before
    /// any wall-clock check and before persisting on pause / exit.
    pub fn checkpoint_wall(&mut self, now_ms: i64) {
        if let Some(start) = self.drive_start_ms {
            self.wall_ms += (now_ms - start).max(0);
            self.drive_start_ms = Some(now_ms);
        }
    }

    /// Elapsed active wall-clock (ms) including the in-flight drive segment.
    fn wall_ms_now(&self, now_ms: i64) -> i64 {
        let live = self
            .drive_start_ms
            .map(|s| (now_ms - s).max(0))
            .unwrap_or(0);
        self.wall_ms + live
    }

    /// Count one completed turn against the run and the given step / attempt.
    pub fn charge_turn(&mut self, step_id: &str, exec_id: &str) {
        self.turns += 1;
        self.steps.entry(step_id.to_string()).or_default().turns += 1;
        *self.attempts.entry(exec_id.to_string()).or_default() += 1;
    }

    /// Charge the token delta for `agent_id`'s latest turn against the run and the
    /// step. `cumulative` is the provider's running session total (0 when the
    /// provider exposes no usage), so the delta is what this turn added.
    pub fn charge_tokens(&mut self, agent_id: &str, step_id: &str, usage: Option<TurnUsage>) {
        let cumulative = usage.map(|u| u.input_tokens + u.output_tokens).unwrap_or(0);
        let seen = self
            .agent_tokens_seen
            .entry(agent_id.to_string())
            .or_insert(0);
        // Deltas only; guard against a non-monotonic report (never negative).
        let delta = cumulative.saturating_sub(*seen);
        *seen = cumulative;
        if delta == 0 {
            return;
        }
        let delta = delta as i64;
        self.tokens += delta;
        self.steps.entry(step_id.to_string()).or_default().tokens += delta;
    }

    /// The first run-level cap this ledger has reached, if any (§11.2). Checked at
    /// each enforcement point; `Some` pauses the run. `>=` because a cap of N
    /// permits N units of spend — reaching N means the next unit would exceed.
    /// Reserved sub-run capacity (§10.3) counts against the caps: the parent must
    /// not spend budget it has already promised to a live sub-run.
    pub fn exceeded(&self, eff: &EffectiveBudgets, now_ms: i64) -> Option<BudgetLimit> {
        if self.turns + self.reserved_turns >= eff.turns {
            return Some(BudgetLimit::Turns);
        }
        if let Some(cap) = eff.tokens {
            if self.tokens + self.reserved_tokens >= cap {
                return Some(BudgetLimit::Tokens);
            }
        }
        if self.wall_ms_now(now_ms) >= eff.wall_clock_mins.saturating_mul(60_000) {
            return Some(BudgetLimit::WallClock);
        }
        None
    }

    /// Turns still spendable under `eff` after committed spend and live reservations
    /// (spec §10.3 budget-fit check). Never negative.
    pub fn remaining_turns(&self, eff: &EffectiveBudgets) -> i64 {
        (eff.turns - self.turns - self.reserved_turns).max(0)
    }

    /// Tokens still spendable, or `None` when the run has no token cap (so a token
    /// reservation is unbounded and never rejected on token grounds).
    pub fn remaining_tokens(&self, eff: &EffectiveBudgets) -> Option<i64> {
        eff.tokens
            .map(|cap| (cap - self.tokens - self.reserved_tokens).max(0))
    }

    /// Reserve a sub-run's budget slice out of the parent ledger (§10.3). The slice
    /// is held until the sub-run finishes and [`Self::release_reservation`] returns
    /// it. Caller has already checked it fits via [`Self::remaining_turns`] /
    /// [`Self::remaining_tokens`].
    pub fn reserve(&mut self, turns: i64, tokens: i64) {
        self.reserved_turns += turns;
        self.reserved_tokens += tokens;
    }

    /// Release a finished sub-run's reservation (§10.3), floored at zero so a
    /// double release or a mismatched slice can never drive the reservation
    /// negative. Pair with [`fold_child_ledger`](super::scheduler) which adds the
    /// sub-run's *actual* spend — the two together replace a reservation with real
    /// consumption.
    pub fn release_reservation(&mut self, turns: i64, tokens: i64) {
        self.reserved_turns = (self.reserved_turns - turns).max(0);
        self.reserved_tokens = (self.reserved_tokens - tokens).max(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budgets(turns: Option<i64>, tokens: Option<i64>, wall: Option<i64>) -> Budgets {
        Budgets {
            turns,
            tokens,
            wall_clock_mins: wall,
            turns_per_attempt: None,
            max_attempts: None,
            spawn_timeout_secs: None,
            turn_start_timeout_secs: None,
            stall_timeout_secs: None,
            nudge_timeout_secs: None,
            tests_timeout_secs: None,
        }
    }

    fn spec_with(budgets: Option<Budgets>) -> Spec {
        Spec {
            version: 1,
            name: "t".into(),
            description: None,
            budgets,
            agents: Default::default(),
            workflow: vec![],
            finalize: None,
        }
    }

    #[test]
    fn resolve_uses_defaults_when_spec_is_silent() {
        let eff = EffectiveBudgets::resolve(&spec_with(None));
        assert_eq!(eff.turns, DEFAULT_TURNS);
        assert_eq!(eff.tokens, None);
        assert_eq!(eff.wall_clock_mins, DEFAULT_WALL_CLOCK_MINS);
        assert_eq!(eff.turns_per_attempt, DEFAULT_TURNS_PER_ATTEMPT);
        assert_eq!(eff.max_attempts, DEFAULT_MAX_ATTEMPTS);
    }

    #[test]
    fn spec_overrides_defaults_field_by_field() {
        let eff = EffectiveBudgets::resolve(&spec_with(Some(budgets(
            Some(12),
            Some(500_000),
            None, // wall_clock left to the default
        ))));
        assert_eq!(eff.turns, 12);
        assert_eq!(eff.tokens, Some(500_000));
        assert_eq!(eff.wall_clock_mins, DEFAULT_WALL_CLOCK_MINS);
    }

    #[test]
    fn step_budgets_override_run_level() {
        let run = EffectiveBudgets::resolve(&spec_with(Some(budgets(Some(50), None, None))));
        let mut sb = budgets(None, None, None);
        sb.turns_per_attempt = Some(3);
        let step = run.for_step(Some(&sb));
        assert_eq!(step.turns_per_attempt, 3);
        // Run-level caps are inherited unchanged.
        assert_eq!(step.turns, 50);
    }

    #[test]
    fn step_budgets_cannot_move_run_caps() {
        // The run-scoped caps drive run-wide enforcement; a step's budgets must
        // not widen or tighten them, even if the step sets turns/tokens/wall.
        let run =
            EffectiveBudgets::resolve(&spec_with(Some(budgets(Some(50), Some(1000), Some(60)))));
        let step = run.for_step(Some(&budgets(Some(5), Some(10), Some(1))));
        assert_eq!(step.turns, 50, "run turn cap frozen");
        assert_eq!(step.tokens, Some(1000), "run token cap frozen");
        assert_eq!(step.wall_clock_mins, 60, "run wall-clock cap frozen");
    }

    #[test]
    fn exceeded_reports_turns_first() {
        let eff = EffectiveBudgets {
            turns: 2,
            tokens: Some(100),
            wall_clock_mins: 10,
            ..Default::default()
        };
        let mut l = Ledger::default();
        assert_eq!(l.exceeded(&eff, 0), None);
        l.charge_turn("s", "e1");
        assert_eq!(l.exceeded(&eff, 0), None, "1 of 2 turns — ok");
        l.charge_turn("s", "e2");
        assert_eq!(
            l.exceeded(&eff, 0),
            Some(BudgetLimit::Turns),
            "reached the cap"
        );
    }

    #[test]
    fn tokens_enforced_only_when_capped() {
        let uncapped = EffectiveBudgets {
            turns: 1000,
            tokens: None,
            ..Default::default()
        };
        let capped = EffectiveBudgets {
            tokens: Some(1000),
            ..uncapped.clone()
        };
        let mut l = Ledger::default();
        l.charge_tokens(
            "a",
            "s",
            Some(TurnUsage {
                input_tokens: 800,
                output_tokens: 400,
            }),
        );
        assert_eq!(l.tokens, 1200);
        assert_eq!(l.exceeded(&uncapped, 0), None, "no token cap → never trips");
        assert_eq!(l.exceeded(&capped, 0), Some(BudgetLimit::Tokens));
    }

    #[test]
    fn token_charges_are_per_turn_deltas() {
        let mut l = Ledger::default();
        // Cumulative session usage grows across turns; the ledger charges deltas.
        l.charge_tokens(
            "a",
            "s",
            Some(TurnUsage {
                input_tokens: 100,
                output_tokens: 0,
            }),
        );
        l.charge_tokens(
            "a",
            "s",
            Some(TurnUsage {
                input_tokens: 250,
                output_tokens: 0,
            }),
        );
        assert_eq!(l.tokens, 250, "delta 100 then 150");
        assert_eq!(l.steps["s"].tokens, 250);
    }

    #[test]
    fn no_usage_never_charges_tokens() {
        let mut l = Ledger::default();
        l.charge_tokens("a", "s", None);
        l.charge_tokens("a", "s", None);
        assert_eq!(l.tokens, 0);
    }

    #[test]
    fn rollup_tracks_step_and_attempt() {
        let mut l = Ledger::default();
        l.charge_turn("plan", "e1");
        l.charge_turn("plan", "e1");
        l.charge_turn("build", "e2");
        assert_eq!(l.turns, 3, "run total");
        assert_eq!(l.steps["plan"].turns, 2);
        assert_eq!(l.steps["build"].turns, 1);
        assert_eq!(l.attempts["e1"], 2);
        assert_eq!(l.attempts["e2"], 1);
    }

    #[test]
    fn wall_clock_accumulates_active_time_across_drives() {
        let eff = EffectiveBudgets {
            wall_clock_mins: 1, // 60_000 ms
            turns: 1000,
            ..Default::default()
        };
        let mut l = Ledger::default();
        l.start_drive(0);
        assert_eq!(l.exceeded(&eff, 30_000), None, "30s of 60s");
        // Pause after 40s of active time.
        l.checkpoint_wall(40_000);
        assert_eq!(l.wall_ms, 40_000);
        // Resume much later (pause time doesn't count): a fresh drive start.
        l.start_drive(1_000_000);
        assert_eq!(l.exceeded(&eff, 1_010_000), None, "40s + 10s = 50s < 60s");
        assert_eq!(
            l.exceeded(&eff, 1_030_000),
            Some(BudgetLimit::WallClock),
            "40s + 30s = 70s > 60s"
        );
    }

    #[test]
    fn patch_adds_to_run_caps() {
        let mut eff = EffectiveBudgets {
            turns: 10,
            tokens: Some(1000),
            wall_clock_mins: 60,
            ..Default::default()
        };
        eff.apply_patch(&budgets(Some(5), Some(500), Some(30)));
        assert_eq!(eff.turns, 15);
        assert_eq!(eff.tokens, Some(1500));
        assert_eq!(eff.wall_clock_mins, 90);
    }

    #[test]
    fn patch_on_unlimited_tokens_is_a_noop() {
        let mut eff = EffectiveBudgets {
            tokens: None,
            ..Default::default()
        };
        eff.apply_patch(&budgets(None, Some(500), None));
        assert_eq!(eff.tokens, None, "adding to unlimited stays unlimited");
    }

    #[test]
    fn reservation_counts_against_caps_and_remaining() {
        let eff = EffectiveBudgets {
            turns: 100,
            tokens: Some(1_000_000),
            wall_clock_mins: 480,
            ..Default::default()
        };
        let mut l = Ledger::default();
        l.charge_turn("orch", "e1"); // 1 turn spent
        l.reserve(30, 500_000); // sub-run slice reserved
        assert_eq!(l.remaining_turns(&eff), 69, "100 - 1 spent - 30 reserved");
        // charge_turn spends no tokens, so only the reservation is deducted.
        assert_eq!(l.remaining_tokens(&eff), Some(500_000));
        // The reservation bites even though the parent itself has spent almost
        // nothing: it must not hand out capacity it already promised.
        let tight = EffectiveBudgets {
            turns: 31,
            ..eff.clone()
        };
        assert_eq!(
            l.exceeded(&tight, 0),
            Some(BudgetLimit::Turns),
            "1 spent + 30 reserved >= 31"
        );
    }

    #[test]
    fn release_then_fold_replaces_reservation_with_actual_spend() {
        let mut parent = Ledger::default();
        parent.reserve(30, 500_000);
        assert_eq!(parent.reserved_turns, 30);
        // Sub-run actually spent 12 turns; release the slice and fold real spend.
        let mut sub = Ledger::default();
        for i in 0..12 {
            sub.charge_turn("s", &format!("e{i}"));
        }
        parent.release_reservation(30, 500_000);
        parent.turns += sub.turns; // mirrors fold_child_ledger
        assert_eq!(parent.reserved_turns, 0, "reservation released");
        assert_eq!(
            parent.turns, 12,
            "only actual spend counts, never the slice"
        );
        assert_eq!(parent.reserved_tokens, 0);
    }

    #[test]
    fn release_is_floored_at_zero() {
        let mut l = Ledger::default();
        l.reserve(5, 0);
        l.release_reservation(10, 100); // over-release
        assert_eq!(l.reserved_turns, 0);
        assert_eq!(l.reserved_tokens, 0);
    }

    #[test]
    fn remaining_tokens_none_when_uncapped() {
        let eff = EffectiveBudgets {
            tokens: None,
            ..Default::default()
        };
        let l = Ledger::default();
        assert_eq!(l.remaining_tokens(&eff), None);
    }

    #[test]
    fn ledger_json_round_trips_persisted_fields() {
        let mut l = Ledger::default();
        l.charge_turn("s", "e1");
        l.charge_tokens(
            "a",
            "s",
            Some(TurnUsage {
                input_tokens: 5,
                output_tokens: 5,
            }),
        );
        l.wall_ms = 1234;
        l.reserve(7, 900);
        let restored = Ledger::from_json(&l.to_json());
        assert_eq!(restored.turns, 1);
        assert_eq!(restored.tokens, 10);
        assert_eq!(restored.wall_ms, 1234);
        assert_eq!(restored.steps["s"].turns, 1);
        // Live reservations survive a pause/resume so the resumed parent still
        // accounts for a sub-run in flight.
        assert_eq!(restored.reserved_turns, 7);
        assert_eq!(restored.reserved_tokens, 900);
        // Transient fields do not survive serialization.
        assert!(restored.agent_tokens_seen.is_empty());
    }
}
