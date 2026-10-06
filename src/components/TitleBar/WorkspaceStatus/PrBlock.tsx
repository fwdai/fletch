import type { GitState, PrChecks, PrSetEntry } from "@/api";
import { Icon } from "@/components/Icon";
import { PR_META, prBadge, repoSlug } from "./derive";

/** The popover's PR block: every PR of the checkout, focused first. Each row is
 *  a state tag, `#N` and title, with the check breakdown while it is open. */
export function PrBlock({
  prs,
  focused,
  git,
}: {
  prs: readonly PrSetEntry[];
  focused: number;
  git: GitState | null;
}) {
  const slug = repoSlug(git?.remote_url);
  return (
    <>
      <div className="ws-pop-div" />
      <div className={`ws-pop-prs ${prs.length > 1 ? "many" : ""}`}>
        {prs.map(({ state, checks }, i) => {
          const isFocused = state.number === focused;
          // Local conflict markers belong to the checkout, which only the
          // focused PR's tint should read.
          const meta = PR_META[prBadge(state, isFocused ? git : null, checks)];
          return (
            <div key={state.number} className="ws-pop-pr">
              <div className="ws-pr-head">
                <span className={`ws-pr-tag pr-${meta.cls}`}>
                  <Icon name={meta.icon} size={11} />
                  {meta.label} PR
                </span>
                <span className="ws-pr-num mono">#{state.number}</span>
                {i === 0 && slug && <span className="ws-pop-repo mono">{slug}</span>}
              </div>
              {state.title && <div className="ws-pr-title">{state.title}</div>}
              {state.state === "open" && checks && checks.total > 0 && (
                <CheckDetail checks={checks} />
              )}
            </div>
          );
        })}
      </div>
    </>
  );
}

function CheckDetail({ checks }: { checks: PrChecks }) {
  return (
    <div className="ws-checkblock">
      <div className="ws-checkbar" role="img" aria-label="check status">
        {checks.passed > 0 && <i className="p" style={{ flex: checks.passed }} />}
        {checks.failed > 0 && <i className="f" style={{ flex: checks.failed }} />}
        {checks.pending > 0 && <i className="w" style={{ flex: checks.pending }} />}
      </div>
      <div className="ws-checkcounts">
        <span className="ok">
          <Icon name="check" size={10} />
          {checks.passed} passed
        </span>
        {checks.failed > 0 && (
          <span className="bad">
            <Icon name="close" size={10} />
            {checks.failed} failed
          </span>
        )}
        {checks.pending > 0 && (
          <span className="pend">
            <span className="ws-spin sm" />
            {checks.pending} running
          </span>
        )}
      </div>
    </div>
  );
}
