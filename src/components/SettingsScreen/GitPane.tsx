import { useEffect, useState } from "react";
import { type GitCommitAction, isCommitAction } from "@/components/RightPanel/primaryActions";
import { Select } from "@/components/ui/Select";
import { TextInput } from "@/components/ui/TextInput";
import { useAppStore } from "@/store";
import { SetGroup, SetHead, SetRow, SetToggle } from "./primitives";

const ACTION_OPTIONS: { value: GitCommitAction; label: string }[] = [
  { value: "agent-commit", label: "Commit" },
  { value: "agent-commit-push", label: "Commit & push" },
  { value: "agent-commit-pr", label: "Commit & open PR" },
];

/** The Select wants string values; the store keeps seconds. `0` = until answered. */
const WAIT_OPTIONS: { value: string; label: string }[] = [
  { value: "120", label: "2 minutes" },
  { value: "600", label: "10 minutes" },
  { value: "0", label: "Until answered" },
];

/** How agents publish: the Git panel's default action, the approval gate and
 *  its wait, draft PRs, and the branch prefix. */
export function GitPane() {
  const gitCommitAction = useAppStore((s) => s.gitCommitAction);
  const setGitCommitAction = useAppStore((s) => s.setGitCommitAction);
  const publishConfirmation = useAppStore((s) => s.publishConfirmation);
  const setPublishConfirmation = useAppStore((s) => s.setPublishConfirmation);
  const publishApprovalWait = useAppStore((s) => s.publishApprovalWait);
  const setPublishApprovalWait = useAppStore((s) => s.setPublishApprovalWait);
  const draftPrs = useAppStore((s) => s.draftPrs);
  const setDraftPrs = useAppStore((s) => s.setDraftPrs);

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Settings · Git"
        title="Git"
        desc="How agents commit, push, and open pull requests on your behalf."
      />

      <SetGroup label="Publishing">
        <SetRow
          title="Default Git action"
          sub="Preselected action in the Git panel. You can still pick another from its menu."
        >
          <Select<GitCommitAction>
            value={gitCommitAction}
            ariaLabel="Default Git action"
            options={ACTION_OPTIONS}
            onChange={(v) => {
              if (isCommitAction(v)) setGitCommitAction(v);
            }}
          />
        </SetRow>
        <SetRow
          title="Ask before publishing"
          sub="Require your approval when an agent decides to push or open a PR on its own."
        >
          <SetToggle
            on={publishConfirmation}
            onClick={() => void setPublishConfirmation(!publishConfirmation)}
          />
        </SetRow>
        <SetRow
          title="Approval wait"
          sub="How long an agent waits for your publish approval. Unanswered requests are denied."
        >
          <Select
            value={String(publishApprovalWait)}
            ariaLabel="Approval wait"
            disabled={!publishConfirmation}
            options={WAIT_OPTIONS}
            onChange={(v) => void setPublishApprovalWait(Number(v))}
          />
        </SetRow>
        <SetRow
          title="Open PRs as drafts"
          sub="Every pull request Fletch opens starts as a draft until you mark it ready."
        >
          <SetToggle on={draftPrs} onClick={() => void setDraftPrs(!draftPrs)} />
        </SetRow>
      </SetGroup>

      <SetGroup label="Branches" last>
        <BranchPrefixRow />
      </SetGroup>
    </div>
  );
}

/** Free-text prefix, committed on blur/Enter. The backend validates and
 *  normalizes; a rejection shows inline and leaves the field editable. */
function BranchPrefixRow() {
  const branchPrefix = useAppStore((s) => s.branchPrefix);
  const setBranchPrefix = useAppStore((s) => s.setBranchPrefix);
  const [draft, setDraft] = useState(branchPrefix);
  const [error, setError] = useState<string | null>(null);

  // Follow the store (hydration, a normalized save) so the field never drifts.
  useEffect(() => {
    setDraft(branchPrefix);
  }, [branchPrefix]);

  const commit = async () => {
    if (draft.trim() === branchPrefix) {
      setDraft(branchPrefix);
      return;
    }
    try {
      await setBranchPrefix(draft);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <SetRow
      title="Branch prefix"
      sub={
        error ??
        "Prepended to every branch an agent creates, e.g. alex/ or feature/. Blank for none."
      }
    >
      <TextInput
        mono
        value={draft}
        placeholder="none"
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        invalid={!!error}
        aria-label="Branch prefix"
        onChange={(e) => {
          setDraft(e.target.value);
          if (error) setError(null);
        }}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
      />
    </SetRow>
  );
}
