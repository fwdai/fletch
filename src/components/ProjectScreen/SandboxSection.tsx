import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { useBoxesUnlocked } from "@/util/boxesUnlocked";
import { useProjectSettings } from "@/util/useProjectSettings";

/** Project-settings key the host's spawn path reads (Rust
 *  `sandbox::placement::KEY`). Absent is automatic. */
const KEY = "sandbox.placement";

type Placement = "auto" | "host" | "box";

function parsePlacement(raw: string | undefined): Placement {
  return raw === "host" || raw === "box" ? raw : "auto";
}

/** Where this project's agents run when the engine in Settings › Sandbox is
 *  Automatic: the isolated VM when it can boot, this Mac for a project that
 *  builds a macOS or iOS app (detected on the host, `native_app`), or
 *  whichever the user pins here. A pinned engine in Settings ignores this. */
export function SandboxSection({
  projectId,
  nativeApp,
}: {
  projectId: string;
  /** The host found an Xcode project, a Podfile, a Tauri app or an
   *  Apple-platform Swift package at the repository root. */
  nativeApp: boolean;
}) {
  const unlocked = useBoxesUnlocked();
  const { settings, save } = useProjectSettings(unlocked ? projectId : null);
  const gate = useGate("projectSettings");
  const boxesEnabled = useAppStore((s) => s.boxesEnabled);
  const value = parsePlacement(settings?.[KEY]);
  // Nothing to place until boxes are rolled out to this user.
  if (!unlocked) return null;

  const hint = !boxesEnabled
    ? "Isolated VMs are off in Settings › Sandbox, so every workspace runs on this Mac."
    : value === "host"
      ? "Every new workspace runs in the macOS sandbox on this Mac."
      : value === "box"
        ? "Every new workspace runs in an isolated VM, waiting for the image if it is still on its way."
        : nativeApp
          ? "This repository builds a macOS or iOS app, which a VM can't build or run, so Automatic keeps its agents on this Mac. Pick Isolated VM to override that."
          : "An isolated VM once it's ready, the macOS sandbox until then.";

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Sandbox</h2>
        <p className="ps-section-lead text-sm">
          Where this project&rsquo;s agents run while the engine in Settings › Sandbox is Automatic.
          Each workspace keeps the sandbox it started in.
        </p>
        {gate && <p className="ps-hint text-sm">{gate}</p>}
      </header>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-placement">
          Where agents run
        </label>
        <select
          id="ps-placement"
          className="ps-input ps-pf-which text-sm"
          value={value}
          disabled={!!gate || settings === null}
          title={gate ?? undefined}
          onChange={(e) => {
            const next = e.target.value as Placement;
            save(KEY, next === "auto" ? null : next);
          }}
        >
          <option value="auto">Automatic</option>
          <option value="box">Isolated VM</option>
          <option value="host">This Mac</option>
        </select>
      </div>
      <p className="ps-hint text-xs">{hint}</p>
    </section>
  );
}
