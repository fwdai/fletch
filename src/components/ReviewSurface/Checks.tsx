// ReviewSurface/Checks — the verification report as scannable pass/fail rows,
// mirroring GitPanel's ChecksSection vocabulary (ok / fail / skip dots). Passing
// checks stay quiet; only failures draw attention. Output tails are progressive
// disclosure — collapsed by default, expandable per row.

import { useState } from "react";
import type { CheckOutcome, CheckResult, VerificationReport } from "../../api";
import { Icon } from "../Icon";

type Tone = "ok" | "fail" | "skip";

const TONE: Record<CheckOutcome, Tone> = {
  passed: "ok",
  failed: "fail",
  timed_out: "fail",
  setup_failed: "fail",
  skipped: "skip",
};

const LABEL: Record<CheckOutcome, string> = {
  passed: "passed",
  failed: "failed",
  timed_out: "timed out",
  setup_failed: "setup failed",
  skipped: "skipped",
};

export function Checks({ verification }: { verification: VerificationReport | null }) {
  // Null verifier (e.g. no sandbox on this host) vs. nothing configured are
  // distinct, honest states — never a fake empty or failed report.
  if (!verification) {
    return <div className="rv-checks-note">Verification isn't available on this host.</div>;
  }
  const ran = verification.checks.filter((c) => c.outcome !== "skipped");
  if (ran.length === 0) {
    return (
      <div className="rv-checks-note">
        No verification commands are configured for this project.
      </div>
    );
  }
  // Failures first so what needs attention is at the top.
  const ordered = [...verification.checks].sort(
    (a, b) => rank(TONE[a.outcome]) - rank(TONE[b.outcome]),
  );
  return (
    <div className="rv-checks">
      {ordered.map((c) => (
        <CheckRow key={c.name} check={c} />
      ))}
    </div>
  );
}

const rank = (t: Tone) => (t === "fail" ? 0 : t === "ok" ? 1 : 2);

function CheckRow({ check }: { check: CheckResult }) {
  const [open, setOpen] = useState(false);
  const tone = TONE[check.outcome];
  const hasTail = check.tail.length > 0;
  return (
    <div className={`rv-check ${tone}`}>
      <button
        type="button"
        className="rv-check-head"
        disabled={!hasTail}
        onClick={() => hasTail && setOpen((v) => !v)}
      >
        <span className={`rv-dot ${tone}`} />
        <span className="rv-check-name">{check.name}</span>
        <span className="rv-check-cmd truncate">{check.command || "—"}</span>
        <span className={`rv-check-status ${tone}`}>{LABEL[check.outcome]}</span>
        {hasTail && <Icon name={open ? "chevD" : "chevR"} size={13} className="rv-check-chev" />}
      </button>
      {open && hasTail && <pre className="rv-check-tail">{check.tail.join("\n")}</pre>}
    </div>
  );
}
