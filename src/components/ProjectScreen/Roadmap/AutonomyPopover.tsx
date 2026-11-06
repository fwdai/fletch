import { useState } from "react";
import { Icon } from "@/components/Icon";
import { SettingsRow, SettingsSection } from "@/components/Settings/SettingsRow";
import { Toggle } from "@/components/Settings/Toggle";
import { IconButton } from "@/components/ui/IconButton";
import { Scale } from "@/components/ui/Scale";
import { Scrim } from "@/components/ui/Scrim";
import { useGate } from "@/store/capabilities";
import type { useProjectSettings } from "@/util/useProjectSettings";
import {
  AUTOQUEUE_KEY,
  CONCURRENCY_CHOICES,
  DEFAULT_MAX_CONCURRENT,
  flagOn,
  MAX_CONCURRENT_KEY,
  parseCap,
  SETTLE_REVIEW_KEY,
} from "./autonomy";

const CAP_STOPS = CONCURRENCY_CHOICES.map((n) => ({ value: String(n), label: String(n) }));

/** The autonomy dial, beside the Accept buttons it relabels: how much of the
 *  roadmap runs without you. Three per-project settings read host-side under
 *  the keys in `autonomy.ts`. Back to the default deletes the row (`save(key,
 *  null)`), so a future change of default reaches old projects too. */
export function AutonomyPopover({ settings, save }: ReturnType<typeof useProjectSettings>) {
  const [open, setOpen] = useState(false);
  const gate = useGate("projectSettings");
  const locked = !!gate || settings === null;
  const all = settings ?? {};
  const autoqueue = flagOn(all[AUTOQUEUE_KEY], false);
  const cap = parseCap(all[MAX_CONCURRENT_KEY]);
  const settleReview = flagOn(all[SETTLE_REVIEW_KEY], true);

  const toggleAutoqueue = (next: boolean) => save(AUTOQUEUE_KEY, next ? "1" : null);

  const pickCap = (next: number) =>
    save(MAX_CONCURRENT_KEY, next === DEFAULT_MAX_CONCURRENT ? null : String(next));

  // On is the default for the PM switch, so *off* is the row that has to exist.
  const toggleSettleReview = (next: boolean) => save(SETTLE_REVIEW_KEY, next ? null : "0");

  return (
    <span className="dd-anchor">
      <IconButton
        tip={open ? undefined : "Autonomy"}
        aria-label="Autonomy"
        aria-expanded={open}
        active={open}
        onClick={() => setOpen(!open)}
      >
        <Icon name="gauge" />
      </IconButton>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <div className="pop rm-autonomy">
            <SettingsSection title="Autonomy">
              <SettingsRow
                label="Queue on accept"
                description="Accepting a proposal sends it straight to the queue."
              >
                <Toggle
                  value={autoqueue}
                  onChange={toggleAutoqueue}
                  disabled={locked}
                  title={gate ?? undefined}
                />
              </SettingsRow>
              <div className="sp-row">
                <div className="sp-l text-base">
                  Runs at once
                  <small>Parallel runs open parallel PRs into one repo.</small>
                </div>
                <Scale
                  stops={CAP_STOPS}
                  value={String(cap)}
                  ariaLabel="Runs at once"
                  disabled={locked}
                  onPick={(v) => pickCap(Number(v))}
                />
              </div>
              <SettingsRow
                label="PM reviews finished runs"
                description="Records what deviated as notes and proposals."
              >
                <Toggle
                  value={settleReview}
                  onChange={toggleSettleReview}
                  disabled={locked}
                  title={gate ?? undefined}
                />
              </SettingsRow>
            </SettingsSection>
          </div>
        </>
      )}
    </span>
  );
}
