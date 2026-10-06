import type { AgentRecord, GitState, PrSetEntry, PrState } from "@/api";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { providerChip } from "@/data/providers";
import { type DotStatus, STATUS_LABEL } from "./derive";
import { PrBlock } from "./PrBlock";
import { StatusDot } from "./StatusDot";

interface Props {
  agent: AgentRecord;
  status: DotStatus;
  git: GitState | null;
  pr: PrState | null;
  /** The checkout's whole PR set, focused first. */
  prs: readonly PrSetEntry[];
  onViewPr: () => void;
  onOpenDiff: () => void;
}

/** The hover/focus details popover: who + what, branch, diff, and the PR block
 *  (every PR of the checkout, each with its check breakdown), plus the two
 *  contextual actions. */
export function Popover({ agent, git, pr, prs, status, onViewPr, onOpenDiff }: Props) {
  const branch = git?.branch ?? agent.repos[0]?.branch ?? null;
  const base = git?.parent_branch ?? null;
  const add = git?.additions ?? 0;
  const rem = git?.deletions ?? 0;
  const fileCount = git?.files.length ?? 0;
  // Nothing to act on when the tree is clean and there's no PR — Diff / Open PR
  // would be dead buttons, so drop the whole actions block (and its divider).
  const showActions = !!pr || add > 0 || rem > 0 || fileCount > 0;

  return (
    <div className="ws-pop" role="dialog">
      <div className="ws-pop-head">
        <StatusDot status={status} big />
        <div className="ws-pop-id">
          <div className="ws-pop-name">{agent.name}</div>
          <div className="ws-pop-sub">
            {STATUS_LABEL[status]}
            {agent.task && <> · {agent.task}</>}
          </div>
        </div>
        <ProviderIcon slug={agent.provider} {...providerChip(agent.provider)} size={22} />
      </div>

      <div className="ws-pop-div" />

      {branch && (
        <div className="ws-pop-row">
          <Icon name="branch" size={12} />
          <span className="mono ws-pop-branch">{branch}</span>
          {base && base !== branch && <span className="ws-pop-base mono">← {base}</span>}
          <span className="ws-pop-ab">
            <span className="up">
              <Icon name="arrowUp" size={10} />
              {git?.ahead ?? 0}
            </span>
            <span className="dn">
              <Icon name="arrowDown" size={10} />
              {git?.behind ?? 0}
            </span>
          </span>
        </div>
      )}

      <div className="ws-pop-row">
        <Icon name="diff" size={12} />
        {add || rem ? (
          <>
            <span className="ws-pop-diff">
              <b className="add">+{add}</b>
              <b className="rem">−{rem}</b>
            </span>
            <span className="ws-pop-dim">
              {fileCount} file{fileCount === 1 ? "" : "s"} changed
            </span>
          </>
        ) : (
          <span className="ws-pop-dim">No uncommitted changes</span>
        )}
      </div>

      {pr && <PrBlock prs={prs} git={git} />}

      {showActions && (
        <>
          <div className="ws-pop-div" />
          <div className="ws-pop-actions">
            {pr?.url ? (
              <button type="button" className="ws-act primary" onClick={onViewPr}>
                <Icon name="external" size={12} /> View pull request
              </button>
            ) : (
              <button type="button" className="ws-act primary" onClick={onOpenDiff}>
                <Icon name="pr" size={12} /> Open pull request
              </button>
            )}
            <button type="button" className="ws-act" onClick={onOpenDiff}>
              <Icon name="diff" size={12} /> Diff
            </button>
          </div>
        </>
      )}
    </div>
  );
}
