import { invoke } from "../invoke";
import type { IsolationReport, PublishApproval } from "../types/sandbox";

export const sandboxApi = {
  // What each engine actually guarantees, for the picker (`sandbox::guarantees`).
  describeSandboxIsolation: () => invoke<IsolationReport[]>("describe_sandbox_isolation"),
  // Whether an agent must get the user's approval before publishing. Backend-owned
  // (`publish_confirmation`); off by default.
  getPublishConfirmation: () => invoke<boolean>("get_publish_confirmation"),
  // Answer one prompt. A request that already timed out is ignored backend-side,
  // so a late answer can never publish anything.
  answerPublishApproval: (id: string, approved: boolean) =>
    invoke<void>("answer_publish_approval", { id, approved }),
  // What is still waiting, oldest first. A `publish:approval-requested` event
  // only reaches the clients connected when it fired, so this is how a window
  // that has just (re)connected to a host learns of one — gate it on
  // `hostSupports("approvals_list")`, since an older host answers `unknown op`.
  listPublishApprovals: () => invoke<PublishApproval[]>("approvals_list"),
};
