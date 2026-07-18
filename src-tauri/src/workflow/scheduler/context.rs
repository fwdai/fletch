use super::*;

/// Everything the drive loop needs, decoupled from the service so it is testable
/// with a `MockDriver`, a temp DB, and no `AppHandle`.
pub(crate) struct RunCtx {
    pub(crate) db: Db,
    pub(crate) driver: Arc<dyn AgentDriver>,
    /// `None` under test — the DB is the source of truth; frontend emits are
    /// skipped.
    pub(crate) app: Option<AppHandle>,
    pub(crate) cancel: Arc<AtomicBool>,
    /// The run's pending-ask flag (§10.4), shared with the [`RunHandle`] so the
    /// comms router can raise it; threaded into each attempt.
    pub(crate) pending_ask: Arc<AtomicBool>,
    pub(crate) deadlines: Deadlines,
    /// The active-run registry (spec §6.1, §10.3), so an orchestrate stage can
    /// launch a composed sub-run's own driver task and register it for the
    /// cancel-cascade. `None` under test — sub-runs are driven detached and the
    /// stage tracks them by polling `wf_run.status`.
    pub(crate) runs: Option<Arc<Mutex<HashMap<String, RunHandle>>>>,
}

/// The `wf_run` columns the walker reads.
pub(crate) struct RunEssentials {
    pub(crate) spec_json: String,
    pub(crate) task: String,
    pub(crate) project_id: String,
    pub(crate) repo_path: String,
    pub(crate) run_dir: String,
    pub(crate) branch: String,
    pub(crate) base_sha: String,
    /// Caller-selected launch base branch, or empty when none was given — the
    /// default PR base at finalization unless the spec pins `finalize.pr_base`.
    pub(crate) base_branch: String,
    pub(crate) status: String,
    pub(crate) budgets_json: String,
    pub(crate) spent_json: String,
    /// The GitHub issue this run was started from (bare number), or `None`.
    /// Drives the finalized PR's `Closes #<n>` trailer.
    pub(crate) issue_ref: Option<String>,
}

/// Whether a completed stage advances the run (`Advance`) or halts it (`Stop` —
/// the stage wrote a terminal `failed`/paused status). A merge stage carries the
/// integrated result as the next block's fork source (`line`); an
/// `integrate: none` stage leaves the line unchanged (`line: None`).
pub(crate) enum StageFlow {
    Advance { line: Option<(String, String)> },
    Stop,
}

/// A parallel child's terminal outcome, from the stage's point of view. Every
/// variant carries the child's own budget ledger so the stage can fold its turn
/// / token spend into the run ledger (§11.2).
pub(crate) enum ChildOutcome {
    /// Gate satisfied. `moved_head` records whether the child committed on its
    /// fork (its code is left there under `integrate: none` — §12.3).
    Success {
        moved_head: bool,
        head: Option<String>,
    },
    /// Errored (autonomous retries exhausted), gate blocked, budget-exceeded, or
    /// an unsupported approval gate — anything that isn't a clean `done`.
    Failure { reason: String },
    /// Superseded before finishing by the stage winding down (a join `any`
    /// winner, or a failed `all` stage).
    Canceled,
}

pub(crate) struct ChildResult {
    pub(crate) step_id: String,
    pub(crate) outcome: ChildOutcome,
    pub(crate) ledger: Ledger,
}

/// Everything one parallel child owns to drive itself on its own task (child
/// tasks are spawned into a [`JoinSet`], so they must be `'static`).
pub(crate) struct ChildCtx {
    pub(crate) db: Db,
    pub(crate) driver: Arc<dyn AgentDriver>,
    pub(crate) app: Option<AppHandle>,
    pub(crate) base_deadlines: Deadlines,
    pub(crate) eff: EffectiveBudgets,
    pub(crate) run_id: String,
    pub(crate) run_task: String,
    pub(crate) step: Step,
    pub(crate) agent_spec: AgentSpec,
    pub(crate) fork_base: String,
    pub(crate) blackboard: PathBuf,
    pub(crate) repo: PathBuf,
    pub(crate) run_repo: PathBuf,
    pub(crate) block_index: usize,
    pub(crate) block_count: usize,
    /// Project test/setup command overrides (spec §9.4), resolved once for the
    /// run and cloned per child so a `tests`-gated child resolves its command the
    /// same way a linear step does. The child builds its own `SandboxTestRunner`
    /// (honoring its own `tests_timeout_secs`) in [`drive_child`].
    pub(crate) test_override: Option<String>,
    pub(crate) setup_override: Option<String>,
    /// The stage's integration mode (§12.3). `Merge` children boundary-commit,
    /// pin, and ferry their work into the run repo (like a linear step) so the
    /// stage can merge their refs; `None` children leave code on their fork.
    pub(crate) integrate: Integrate,
    /// An orchestrator note folded into the child's *first* prompt (spec §10.2 —
    /// `retry_child` guidance). Threaded directly rather than via a queued
    /// message so the fresh attempt is guaranteed to carry it. `None` for the
    /// common case.
    pub(crate) extra_note: Option<String>,
    /// The launch generation for this child's `step_id` (spec §10.2). `retry_child`
    /// bumps it and spawns a replacement; the stage ignores results from any
    /// superseded (lower-generation) attempt so a stale finish of the cancelled
    /// task can't win the join. Always `0` outside an orchestrate stage.
    pub(crate) generation: u64,
    /// Set by the stage to wind this child down (loser cancellation, §6.6).
    pub(crate) stage_cancel: Arc<AtomicBool>,
}

/// The stage-visible status of an orchestrate child, for the join decision.
#[derive(Clone)]
pub(crate) enum ChildStatus {
    Success,
    Failure(String),
    /// The orchestrator dropped it with `skip_child` — satisfied, not a failure.
    Skipped,
}

/// Run-wide invariants every step attempt reads, bundled so the walker and the
/// loop executor share a single [`execute_step`] without a dozen positional args.
pub(crate) struct StepEnv<'a> {
    pub(crate) repo: &'a Path,
    pub(crate) run_repo: &'a Path,
    pub(crate) blackboard: &'a Path,
    pub(crate) eff: &'a EffectiveBudgets,
    pub(crate) test_override: &'a Option<String>,
    pub(crate) setup_override: &'a Option<String>,
    pub(crate) run_task: &'a str,
    pub(crate) spec_name: &'a str,
    /// The run's base commit — the fork point the ferried diff in an approval
    /// gate's review evidence is taken against (spec §9).
    pub(crate) base_sha: &'a str,
    /// The run's launch-time file attachments (durable, read-only). Rendered into
    /// the entry step's prompt only (see `execute_step`); empty for stages that
    /// can't be the run entry.
    pub(crate) launch_attachments: &'a [String],
}

/// What executing one step (through its attempt/retry lifecycle) resolved to.
pub(crate) enum StepFlow {
    /// Gate met and ferried into the run repo. `head_ref` is the fork source for
    /// whatever comes next; `exec_id` is the durable record.
    Done { exec_id: String, head_ref: String },
    /// A loop's `until` step ended without a `done` verdict (revise / blocked /
    /// missing) — the loop iterates again rather than pausing. Returned only when
    /// `is_until` is set.
    LoopContinue,
    /// The run reached a paused or terminal state; the status row is already
    /// written and the drive loop must return.
    Halt,
}

/// Whether a loop block completed (advance to the next) or halted the run.
pub(crate) enum BlockFlow {
    Advance,
    Halt,
}

/// The scheduler cursor (spec §6.4): the index into the top-level block sequence
/// plus, for any loop entered, its current iteration keyed by the loop's
/// top-level block index. A run's `spec_json` is immutable after launch, so the
/// index is a stable key. The old `{ "index": N }` shape still deserializes
/// (`iterations` defaults empty).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Cursor {
    #[serde(default)]
    pub(crate) index: i64,
    #[serde(default)]
    pub(crate) iterations: std::collections::BTreeMap<String, u32>,
    /// In-progress state of a code-producing parallel merge (§12.3). Present only
    /// while a `integrate: merge` stage is mid-merge or paused on a conflict; the
    /// cursor `index` still points at that stage until it finalizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) merge: Option<MergeCursor>,
}

/// The resumable state of a merge stage (§12.3): which children remain to merge
/// (in spec order) and, if paused, the recorded conflict.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct MergeCursor {
    pub(crate) block_index: usize,
    /// `(step_id, ferried_ref)` children not yet merged, in spec order.
    pub(crate) remaining: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) conflict: Option<ConflictInfo>,
}

/// A recorded merge conflict awaiting resolution (§12.3 modes a/c).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ConflictInfo {
    /// The child whose merge conflicted.
    pub(crate) step_id: String,
    pub(crate) files: Vec<String>,
    /// The committed conflict snapshot a mode-(a) resolution step forks from.
    pub(crate) conflict_ref: String,
    /// Chosen by `wf_resolve_conflict`: `"agent"` (mode a) or `"human"` (mode c).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resolution: Option<String>,
}
