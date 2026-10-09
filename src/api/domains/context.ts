import { invoke } from "../invoke";
import type {
  AssertionInput,
  CompileQuery,
  ContextBootstrap,
  ContextOverview,
  DismissReason,
  EntityInput,
  LinkChange,
  ProposalVerdict,
} from "../types/context";

/** The project context layer's human side (`context_*`,
 *  crates/fletch-core/src/commands/context.rs). Every write makes the host
 *  emit `context:changed`, so a tab that subscribes reloads rather than
 *  patching state from the answer — see `useContextOverview`. */
export const contextApi = {
  /** The toggles, the graph, the pending proposals and the stats, in one read. */
  contextOverview: (projectId: string) =>
    invoke<ContextOverview>("context_overview", { projectId }),
  /** What an agent would be served for `query`, as markdown text. */
  contextPreview: (projectId: string, query: CompileQuery) =>
    invoke<string>("context_preview", { projectId, query }),
  /** Create an entity, or revise one when `input.id` is set. Resolves to the id. */
  contextRecordEntity: (projectId: string, input: EntityInput) =>
    invoke<string>("context_record_entity", { projectId, input }),
  /** Record a confirmed assertion. `input.supersedes` (with reasoning) is how a
   *  decision is changed: assertions are immutable. Resolves to the id. */
  contextRecordAssertion: (projectId: string, input: AssertionInput) =>
    invoke<string>("context_record_assertion", { projectId, input }),
  /** Hide an assertion that was never right; the reason is required. */
  contextRetract: (projectId: string, assertionId: string, reason: string) =>
    invoke<void>("context_retract", { projectId, assertionId, reason }),
  /** Close a `contradicts` edge between `a` and `b` with a ruling; the
   *  reasoning is required. Neither side changes — retract or Change do that. */
  contextResolveContradiction: (projectId: string, a: string, b: string, reasoning: string) =>
    invoke<void>("context_resolve_contradiction", { projectId, a, b, reasoning }),
  contextArchiveEntity: (projectId: string, entityId: string) =>
    invoke<void>("context_archive_entity", { projectId, entityId }),
  /** `from`'s edges move to `into`; `from` stays behind as `merged`. */
  contextMergeEntities: (projectId: string, from: string, into: string) =>
    invoke<void>("context_merge_entities", { projectId, from, into }),
  contextLink: (projectId: string, change: LinkChange) =>
    invoke<void>("context_link", { projectId, change }),
  /** Record the modules of the project's primary repo at `HEAD` (only slugs
   *  the project has never had), and get the mapping session's task. */
  contextBootstrap: (projectId: string) =>
    invoke<ContextBootstrap>("context_bootstrap", { projectId }),
  /** Accept (resolves to the recorded id) or dismiss (needs a reason) a
   *  pending proposal. */
  contextRuleProposal: (
    projectId: string,
    proposalId: string,
    verdict: ProposalVerdict,
    dismissReason?: DismissReason,
  ) =>
    invoke<string | null>("context_rule_proposal", {
      projectId,
      proposalId,
      verdict,
      dismissReason: dismissReason ?? null,
    }),
  /** Dismiss every pending proposal as trivial; resolves to how many. */
  contextDismissAll: (projectId: string) => invoke<number>("context_dismiss_all", { projectId }),
};
