import {
  EcosystemBadge,
  persistRunOverrides,
  RunConfigEditor,
  reconcileOverrides,
  type SetupRow,
  withFallbackRows,
} from "@/components/RunConfig";
import { SetGroup, SetHead, SetRow, SetToggle } from "@/components/SettingsScreen/primitives";
import { useGate } from "@/store/capabilities";
import { useProjectSettings } from "@/util/useProjectSettings";
import { EnvVars } from "./EnvVars";

/** Project-settings key the host's turn-end hook reads (Rust
 *  `VERIFY_ON_TURN_END_KEY`). Keep the string in sync with that constant. */
const VERIFY_KEY = "verify.on_turn_end";

interface Props {
  projectId: string;
  repoPath: string;
  rows: SetupRow[];
  ecosystem: string | null;
  overrides: Record<string, string>;
  onOverrides: (next: Record<string, string>) => void;
}

/** The run configuration every agent in the project inherits, its `.env`
 *  membrane, and the opt-in checks after each turn. Unlike the Run panel
 *  sheet — which stages a draft and applies on restart — this edits the
 *  project defaults directly, so changes autosave per row. */
export function RunPane({
  projectId,
  repoPath,
  rows: detected,
  ecosystem,
  overrides,
  onOverrides,
}: Props) {
  // Every core field is shown, detected or not — a row the detector couldn't
  // fill (or an unrecognized stack) should be configurable, not hidden.
  const rows = withFallbackRows(detected);
  // The host's verifier and Run panel read these rows.
  const gate = useGate("projectSettings");
  const { settings, save } = useProjectSettings(projectId);
  // OFF by default: it costs a full install/test/lint pass per turn.
  const verify = settings?.[VERIFY_KEY] === "1" || settings?.[VERIFY_KEY] === "true";

  // Reconcile a single edit against the detected rows and persist it. Same
  // logic the Run panel runs on Apply, just committed immediately.
  const commit = (next: Record<string, string>) => {
    const { cleaned, toSet, toDelete } = reconcileOverrides(rows, overrides, next);
    persistRunOverrides(projectId, toSet, toDelete);
    onOverrides(cleaned);
  };

  const onChange = (id: string, value: string) => commit({ ...overrides, [id]: value });
  const onRevert = (id: string) => {
    const next = { ...overrides };
    delete next[id];
    commit(next);
  };

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Project settings · Run"
        title="Run"
        desc="What every agent in this project runs, and with what."
      />

      <SetGroup
        label="Commands"
        aside={
          <span className="ps-eco text-xs">
            <EcosystemBadge ecosystem={ecosystem} />
          </span>
        }
        sub={gate}
      >
        {!gate && (
          <RunConfigEditor
            rows={rows}
            draft={overrides}
            scope="project"
            onChange={onChange}
            onRevert={onRevert}
          />
        )}
      </SetGroup>

      <EnvVars projectId={projectId} repoPath={repoPath} />

      <SetGroup label="Checks" last>
        <SetRow
          title="Run checks after each turn"
          sub="Install, test and lint from above, after every turn."
        >
          <SetToggle
            on={verify}
            disabled={!!gate || settings === null}
            tip={gate ?? undefined}
            onClick={() => save(VERIFY_KEY, verify ? null : "1")}
          />
        </SetRow>
      </SetGroup>
    </div>
  );
}
