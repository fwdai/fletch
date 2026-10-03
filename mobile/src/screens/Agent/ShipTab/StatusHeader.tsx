import type { GitState } from "@desktop/api/types/git";
import type { PrState } from "@desktop/api/types/pr";
import { Icon } from "@desktop/components/Icon";
import type { ShipStrip } from "./derive";

/** The tinted strip at the top of the Ship tab: where the work stands, in a
 *  colour before any word is read. A `working` strip carries the spinner the
 *  chat uses while the agent is on a playbook. */
export function StatusHeader({
  strip,
  git,
  pr,
}: {
  strip: ShipStrip;
  git: GitState | null | undefined;
  pr: PrState | null | undefined;
}) {
  const adds = git?.additions ?? 0;
  const dels = git?.deletions ?? 0;
  return (
    <div className={`ship-hdr k-${strip.kind}`}>
      {strip.kind === "working" ? (
        <span className="working-dots">
          <i />
          <i />
          <i />
        </span>
      ) : (
        <span className="hdr-dot" />
      )}
      <span className="bn">{strip.text}</span>
      {strip.sub && <span className="base mono">{strip.sub}</span>}
      <span className="grow" />
      {strip.diff && (adds > 0 || dels > 0) && (
        <span className="hdr-diff mono">
          {adds > 0 && <span className="add">+{adds}</span>}
          {dels > 0 && <span className="rem">−{dels}</span>}
        </span>
      )}
      {strip.prLink && pr?.url && (
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
