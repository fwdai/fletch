import { useEffect, useState } from "react";
import { Toggle } from "@/components/Settings/Toggle";
import {
  deleteProjectSetting,
  getProjectSettings,
  setProjectSetting,
} from "@/storage/projectSettings";

/** Project-settings key the backend turn-end hook reads (Rust
 *  `VERIFY_ON_TURN_END_KEY`). Keep the string in sync with that constant. */
const KEY = "verify.on_turn_end";

/** Opt-in: run the project's checks after each ad-hoc agent turn so its Mission
 *  Control card arrives with a tests verdict. OFF by default — it costs a full
 *  install/test/lint pass per turn, so it's the user's call per project. */
export function VerifySection({ projectId }: { projectId: string }) {
  const [on, setOn] = useState(false);

  useEffect(() => {
    let cancelled = false;
    getProjectSettings(projectId)
      .then((all) => {
        if (!cancelled) setOn(all[KEY] === "1" || all[KEY] === "true");
      })
      .catch((e) => console.error("load verify.on_turn_end failed", e));
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  const toggle = (next: boolean) => {
    setOn(next);
    const write = next
      ? setProjectSetting(projectId, KEY, "1")
      : deleteProjectSetting(projectId, KEY);
    write.catch((e) => console.error("save verify.on_turn_end failed", e));
  };

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
        <Toggle value={on} onChange={toggle} />
      </div>
    </section>
  );
}
