import { Toggle } from "@/components/Settings/Toggle";
import { useGate } from "@/store/capabilities";
import { useProjectSettings } from "@/util/useProjectSettings";

/** Project-settings key the host's turn-end hook reads (Rust
 *  `VERIFY_ON_TURN_END_KEY`). Keep the string in sync with that constant. */
const KEY = "verify.on_turn_end";

/** Opt-in: run the project's checks after each ad-hoc agent turn so its Mission
 *  Control card arrives with a tests verdict. OFF by default — it costs a full
 *  install/test/lint pass per turn, so it's the user's call per project. The
 *  host runs the check, so this edits the host the UI is driving. */
export function VerifySection({ projectId }: { projectId: string }) {
  const { settings, save } = useProjectSettings(projectId);
  const gate = useGate("projectSettings");
  const on = settings?.[KEY] === "1" || settings?.[KEY] === "true";

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Verify on turn end</h2>
        <p className="ps-section-lead text-sm">
          When on, each ad-hoc agent&rsquo;s turn end runs your project&rsquo;s install / test /
          lint checks on its checkout, so its Mission Control card shows a tests verdict. Off by
          default — it runs a full check pass after every turn.
        </p>
      </header>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-verify">
          Run checks after each turn
        </label>
        <Toggle
          value={on}
          onChange={(next) => save(KEY, next ? "1" : null)}
          disabled={!!gate || settings === null}
          title={gate ?? undefined}
        />
      </div>
    </section>
  );
}
