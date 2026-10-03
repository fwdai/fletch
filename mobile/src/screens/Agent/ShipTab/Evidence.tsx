import type { AgentRecord } from "@desktop/api/types/agent";
import type { GitState } from "@desktop/api/types/git";
import type { PrChecks, PrComments, PrState } from "@desktop/api/types/pr";
import type { VerificationReport } from "@desktop/api/types/verify";
import { Icon, type IconName } from "@desktop/components/Icon";
import { type CSSProperties, type ReactNode, useState } from "react";
import { openExternal } from "../../../lib/links";
import { ChecksList } from "./ChecksList";
import { Comments } from "./Comments";
import { checksSummary } from "./checks";
import { testsEvidence, testsText } from "./verification";

type Tone = "ok" | "bad" | "warn" | "mute" | "acc";

const TONE: Record<Tone, CSSProperties> = {
  ok: {
    background: "color-mix(in oklch, var(--success), transparent 86%)",
    color: "var(--success)",
  },
  bad: {
    background: "color-mix(in oklch, var(--danger), transparent 86%)",
    color: "var(--danger)",
  },
  warn: { background: "color-mix(in oklch, var(--warn), transparent 86%)", color: "var(--warn)" },
  acc: {
    background: "color-mix(in oklch, var(--accent), transparent 86%)",
    color: "var(--accent)",
  },
  mute: { background: "var(--bg-2)", color: "var(--fg-2)" },
};

/** One evidence row: an icon square, the fact, and how to get closer to it —
 *  a tap into the app, out to GitHub, or an unfold under the row. A row with
 *  no `onTap` is a statement, drawn muted. */
function Row({
  icon,
  tone,
  text,
  sub,
  onTap,
  trailing,
}: {
  icon: IconName;
  tone: Tone;
  text: string;
  sub?: string;
  onTap?: () => void;
  trailing?: "chev" | "external" | "open" | "closed";
}) {
  const body = (
    <>
      <span className="pm" style={TONE[tone]}>
        <Icon name={icon} size={11} />
      </span>
      <div className="main">
        <div className="lbl">{text}</div>
        {sub && <div className="sub">{sub}</div>}
      </div>
      {trailing === "chev" && <Icon name="chevR" size={14} className="chev" />}
      {trailing === "external" && <Icon name="external" size={13} className="chev" />}
      {trailing === "open" && <Icon name="chevU" size={14} className="chev" />}
      {trailing === "closed" && <Icon name="chevD" size={14} className="chev" />}
    </>
  );
  return onTap ? (
    <button type="button" className="row ship-row" onClick={onTap}>
      {body}
    </button>
  ) : (
    <div className="row ship-row muted">{body}</div>
  );
}

/** What there is to show for the checkout: only the rows that have something
 *  to say, so a clean tree with no PR renders no card at all. */
export function Evidence({
  agent,
  git,
  pr,
  checks,
  comments,
  report,
  base,
  onShowChanges,
  onDelegated,
}: {
  agent: AgentRecord;
  git: GitState | null | undefined;
  pr: PrState | null | undefined;
  checks: PrChecks | null | undefined;
  comments: PrComments | null | undefined;
  report: VerificationReport | undefined;
  base: string;
  onShowChanges: () => void;
  onDelegated?: () => void;
}) {
  const [checksOpen, setChecksOpen] = useState(false);
  const [reviewOpen, setReviewOpen] = useState(false);
  const files = git?.files.length ?? 0;
  const tests = testsEvidence(report);
  const summary = pr?.state === "open" ? checksSummary(checks) : null;
  const unresolved = pr?.state === "open" ? (comments?.unresolved ?? []) : [];
  const behind = git?.behind ?? 0;

  const rows: ReactNode[] = [];
  if (files > 0) {
    rows.push(
      <Row
        key="changes"
        icon="diff"
        tone="warn"
        text={`${files} file${files === 1 ? "" : "s"} · +${git?.additions ?? 0} −${git?.deletions ?? 0}`}
        onTap={onShowChanges}
        trailing="chev"
      />,
    );
  }
  if (tests) {
    rows.push(
      <Row
        key="tests"
        icon={tests === "passed" ? "check" : "close"}
        tone={tests === "passed" ? "ok" : "bad"}
        text={testsText(tests)}
      />,
    );
  }
  if (pr) {
    rows.push(
      <Row
        key="pr"
        icon={pr.state === "merged" ? "merge" : "pr"}
        tone={pr.state === "open" ? "acc" : "mute"}
        text={`#${pr.number} · ${pr.title}`}
        onTap={() => openExternal(pr.url)}
        trailing="external"
      />,
    );
  }
  if (summary && checks) {
    rows.push(
      <Row
        key="checks"
        icon={checks.failed > 0 ? "close" : checks.pending > 0 ? "loop" : "check"}
        tone={checks.failed > 0 ? "bad" : checks.pending > 0 ? "mute" : "ok"}
        text={`Checks · ${summary}`}
        onTap={() => setChecksOpen((v) => !v)}
        trailing={checksOpen ? "open" : "closed"}
      />,
    );
    if (checksOpen) rows.push(<ChecksList key="checks-list" checks={checks} />);
  }
  if (unresolved.length > 0) {
    rows.push(
      <Row
        key="review"
        icon="user"
        tone="warn"
        text={`Review · ${unresolved.length} unresolved`}
        onTap={() => setReviewOpen((v) => !v)}
        trailing={reviewOpen ? "open" : "closed"}
      />,
    );
    if (reviewOpen) {
      rows.push(
        <Comments key="comments" agent={agent} threads={unresolved} onDelegated={onDelegated} />,
      );
    }
  }
  if (behind > 0) {
    rows.push(
      <Row
        key="base"
        icon="arrowDown"
        tone="mute"
        text={`Behind ${base} by ${behind}`}
        sub={`${base} has new commits this branch does not have yet`}
      />,
    );
  }
  if (git && !git.has_origin) {
    rows.push(
      <Row key="remote" icon="fetch" tone="mute" text="No remote · publishing creates one" />,
    );
  }
  if (rows.length === 0) return null;
  return <div className="card ship-evidence">{rows}</div>;
}
