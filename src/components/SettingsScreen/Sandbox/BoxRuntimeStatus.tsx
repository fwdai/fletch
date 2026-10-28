import { useEffect } from "react";
import { useAppStore } from "@/store";

/** The box runtime probe as a compact status list — helper, dylibs,
 *  hypervisor, arch, image — with the probe's own problem sentences under it.
 *  Shown in Settings › Developer under the box toggle while it is on; probes
 *  on mount, so flipping the toggle answers at once. */
export function BoxRuntimeStatus() {
  const probe = useAppStore((s) => s.boxProbe);
  const refresh = useAppStore((s) => s.refreshBoxProbe);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  if (!probe) return <div className="set-row-s text-sm">Checking the box runtime…</div>;

  const items: [string, string][] = [
    ["Helper", probe.helper ?? "not found"],
    ["Libraries", probe.lib_dir ?? "not found"],
    ["Hypervisor", probe.hypervisor ? "available" : "unavailable"],
    ["Architecture", probe.arch_ok ? "Apple silicon" : "Intel (not supported)"],
    [
      "Image",
      probe.image.phase === "installed"
        ? `v${probe.image.version ?? probe.image.pinned_version}`
        : probe.image.phase.replace("_", " "),
    ],
  ];

  return (
    <div className="set-row-s text-sm">
      <dl className="set-box-probe mono">
        {items.map(([label, value]) => (
          <div key={label}>
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
      {probe.ok ? (
        <p>Ready: this Mac can boot a box.</p>
      ) : (
        <ul className="set-box-problems">
          {probe.problems.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      )}
    </div>
  );
}
