use serde::Deserialize;
use serde_json::Value;

use crate::roadmap::proposals::ProposalPatch;

/// `roadmap_list` args. Everything optional: no args at all is the common call.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListArgs {
    /// Keep only these statuses. Absent means every row.
    #[serde(default)]
    pub(super) status: Option<Vec<String>>,
}

/// `roadmap_propose` args.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeArgs {
    #[serde(default)]
    pub(super) items: Vec<ProposedItem>,
}

/// One proposed ticket, as the agent writes it. Deliberately *not* [`NewItem`]:
/// the agent may not choose `status` or `source` (they are what makes this a
/// proposal), and unknown fields are rejected rather than silently dropped —
/// a misspelled `horizen` must fail loudly, not put the item in the backlog.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposedItem {
    #[serde(default)]
    pub(super) title: String,
    #[serde(default)]
    pub(super) why: String,
    #[serde(default)]
    pub(super) horizon: Option<String>,
    #[serde(default)]
    pub(super) area: Option<String>,
    #[serde(default)]
    pub(super) accept: Vec<String>,
    #[serde(default)]
    pub(super) deps: Vec<String>,
}

/// `roadmap_propose_update` args. `patch` is the [`ProposalPatch`] shape —
/// `deny_unknown_fields` there is what refuses `status`/`code`/`source` and
/// every run back-link with a precise error.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeUpdateArgs {
    pub(super) code: String,
    pub(super) patch: ProposalPatch,
    /// One honest sentence on why — quoted on the card next to the diff.
    #[serde(default)]
    pub(super) note: Option<String>,
}

/// `roadmap_propose_discard` args. Unlike the update's optional note, the
/// `reason` is required: asking to remove work someone agreed to is exactly
/// the ask that must explain itself.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeDiscardArgs {
    pub(super) code: String,
    #[serde(default)]
    pub(super) reason: String,
}

/// `roadmap_propose_order` args: the whole new sequence, and why. `codes` must
/// name every orderable item on the board — the refusal says which are missing
/// or don't belong (see [`order_proposals::validate_order`]).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeOrderArgs {
    #[serde(default)]
    pub(super) codes: Vec<String>,
    /// One honest sentence on why this order — the user reads it above the board.
    #[serde(default)]
    pub(super) note: Option<String>,
}

/// `roadmap_note` args: which item, and the observation. Both required — a note
/// with no text is the one thing this op cannot record.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NoteArgs {
    pub(super) code: String,
    #[serde(default)]
    pub(super) note: String,
}

/// `roadmap_hold` args: what to stop, and why. Both required — a brake with no
/// reason leaves the user a button and no way to know what it undoes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HoldArgs {
    /// An item code, or the literal `"project"` for the whole board.
    pub(super) scope: String,
    #[serde(default)]
    pub(super) reason: String,
}

/// `roadmap_brief` args: none. An explicit empty shape rather than ignoring
/// whatever arrives, so an agent that guesses at a filter (`{"section":"vision"}`)
/// is told this op takes the whole document instead of silently getting it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BriefArgs {}

/// `roadmap_propose_brief_update` args: the whole brief the PM wants the project
/// to have, and one line on what changed. Whole rather than a patch — see
/// [`crate::roadmap::memory::BriefProposal`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProposeBriefArgs {
    #[serde(default)]
    pub(super) content: String,
    /// What changed and why — quoted on the tab's bar next to Accept/Decline.
    #[serde(default)]
    pub(super) note: Option<String>,
}

/// The `scope` value that means "the whole board" rather than one item. A literal
/// rather than a second op, because the two holds are one decision at two
/// altitudes and the PM should pick the altitude, not the tool.
pub(super) const PROJECT_SCOPE: &str = "project";

/// Decode `args` into an op's shape. A missing `args` arrives as JSON null,
/// which is the same as `{}` for every op here.
pub(super) fn parse_args<T: Default + serde::de::DeserializeOwned>(
    args: &Value,
) -> Result<T, String> {
    if args.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(args.clone()).map_err(|e| e.to_string())
}

/// Decode `args` for an op with required fields, where a missing `args` is a
/// mistake worth naming rather than an empty default.
pub(super) fn parse_required<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, String> {
    if args.is_null() {
        return Err("`args` are required".into());
    }
    serde_json::from_value(args.clone()).map_err(|e| e.to_string())
}

/// Trim, and treat an empty string as absent — an agent that fills a field with
/// `""` means "no value", and storing that would show as an empty tag.
pub(super) fn clean(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Trim a list and drop the blanks.
pub(super) fn clean_list(v: &[String]) -> Vec<String> {
    v.iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The valid spellings of an enum, for an error message the agent can act on.
pub(super) fn one_of(values: &[&str]) -> String {
    values.join(" | ")
}
