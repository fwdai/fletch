import type { GitState } from "@desktop/api/types/git";
import type { PrChecks, PrState } from "@desktop/api/types/pr";
import { Icon } from "@desktop/components/Icon";
import { describeGitHeader } from "./derive";

/** The tinted strip at the top of the Git tab: the checkout's state in a
 *  colour before any word is read. */
export function StatusHeader({
  git,
  pr,
  checks,
  branch,
  base,
}: {
  git: GitState | null | undefined;
  pr: PrState | null | undefined;
  checks: PrChecks | null | undefined;
  branch: string;
  base: string;
}) {
  const h = describeGitHeader(git, pr, checks, branch, base);
  const adds = git?.additions ?? 0;
  const dels = git?.deletions ?? 0;
  return (
    <div className={`git-hdr k-${h.kind}`}>
      {h.dot && <span className="hdr-dot" />}
      {h.pill && <span className="pill">{h.pill}</span>}
      <span className="bn mono">{h.text}</span>
      {h.sub && <span className="base mono">{h.sub}</span>}
      <span className="grow" />
      {h.diff && (adds > 0 || dels > 0) && (
        <span className="hdr-diff mono">
          {adds > 0 && <span className="add">+{adds}</span>}
          {dels > 0 && <span className="rem">−{dels}</span>}
        </span>
      )}
      {h.prLink && pr?.url && (
        <a
          href={pr.url}
          target="_blank"
          rel="noreferrer"
          className="hdr-ext"
          aria-label="View on GitHub"
        >
          <Icon name="external" size={13} />
        </a>
      )}
    </div>
  );
}
