import type { AgentRecord } from "@desktop/api/types/agent";
import type { GitState } from "@desktop/api/types/git";
import type { PrChecks, PrComments, PrState } from "@desktop/api/types/pr";
import type { VerificationReport } from "@desktop/api/types/verify";
import { Icon, type IconName } from "@desktop/components/Icon";
import type {
  LadderRow,
  RungKey,
  RungMark,
  RungTone,
} from "@desktop/components/RightPanel/GitPanel/ladderModel";
import { type CSSProperties, type ReactNode, useState } from "react";
import { openExternal } from "../../../lib/links";
import { ChecksList } from "./ChecksList";
import { Comments } from "./Comments";
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

/** A ladder row's colour here: its attention colour when it has one, else its
 *  mark. */
const TONE_OF_RUNG: Record<RungTone, Tone> = {
  warn: "warn",
  att: "warn",
  danger: "bad",
  info: "acc",
  merged: "acc",
  neutral: "mute",
};
const TONE_OF_MARK: Record<RungMark, Tone> = {
  done: "ok",
  running: "mute",
  attention: "warn",
  idle: "mute",
};
const ICON: Record<RungKey, IconName> = {
  changes: "diff",
  commits: "commit",
  pr: "pr",
  checks: "check",
  review: "user",
};

/** What there is to show for the checkout: the desktop ladder's rows that have
 *  something to say (an idle row is a stage not reached, or nothing to read),
 *  the turn-end tests, and two facts the phone's footer can't act on — a base
 *  that moved on, and no remote — so a clean tree with no PR renders no card. */
export function Evidence({
  agent,
  ladder,
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
  ladder: LadderRow[];
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
  const tests = testsEvidence(report);
  const unresolved = comments?.unresolved ?? [];
  const behind = git?.behind ?? 0;

  /** How to get closer to a row's fact: the diff, the PR on GitHub, or the
   *  checks and threads unfolded under it. */
  const tap = (key: RungKey): Pick<Parameters<typeof Row>[0], "onTap" | "trailing"> => {
    switch (key) {
      case "changes":
        return git?.files.length ? { onTap: onShowChanges, trailing: "chev" } : {};
      case "pr":
        return pr ? { onTap: () => openExternal(pr.url), trailing: "external" } : {};
      case "checks":
        return checks?.runs.length
          ? { onTap: () => setChecksOpen((v) => !v), trailing: checksOpen ? "open" : "closed" }
          : {};
      case "review":
        return unresolved.length
          ? { onTap: () => setReviewOpen((v) => !v), trailing: reviewOpen ? "open" : "closed" }
          : {};
      default:
        return {};
    }
  };

  const rows: ReactNode[] = [];
  for (const r of ladder) {
    if (r.mark === "idle") continue;
    rows.push(
      <Row
        key={r.key}
        icon={r.key === "pr" && pr?.state === "merged" ? "merge" : ICON[r.key]}
        tone={r.tone ? TONE_OF_RUNG[r.tone] : TONE_OF_MARK[r.mark]}
        text={r.title}
        sub={r.summary}
        {...tap(r.key)}
      />,
    );
    if (r.key === "checks" && checksOpen && checks) {
      rows.push(<ChecksList key="checks-list" checks={checks} />);
    }
    if (r.key === "review" && reviewOpen && unresolved.length > 0) {
      rows.push(
        <Comments key="comments" agent={agent} threads={unresolved} onDelegated={onDelegated} />,
      );
    }
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
