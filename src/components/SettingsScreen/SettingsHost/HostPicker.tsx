import { EnvironmentDot } from "@/components/HostChip/EnvironmentDot";
import { Select, type SelectOption } from "@/components/ui/Select";
import { useAppStore } from "@/store";
import { connectionLabel } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";
import { useSettingsHost } from "./context";

/** This Mac first, then the paired hosts by name, so the list holds still as
 *  connections come and go. */
const order = (a: EnvironmentEntry, b: EnvironmentEntry) =>
  a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "local" ? -1 : 1;

const option = (env: EnvironmentEntry): SelectOption<string> => ({
  value: env.id,
  label: env.name,
  // This Mac is always there; a host's line says whether it can be reached.
  hint: env.kind === "remote" ? connectionLabel(env) : undefined,
  icon: <EnvironmentDot env={env} />,
});

/** "Settings for: This Mac", under the head of each pane of host-owned
 *  settings. Lists This Mac and every paired host, an offline one too: its
 *  pane shows what was last known, and its gates close what it cannot answer.
 *  Renders nothing while no host is paired. */
export function HostPicker() {
  const { hostId, choose } = useSettingsHost();
  const environments = useAppStore((s) => s.environments);
  const entries = Object.values(environments);
  if (entries.length < 2) return null;

  return (
    <div className="set-host-pick flex-center text-sm">
      <span className="set-host-pick-label">Settings for</span>
      <Select<string>
        value={hostId}
        ariaLabel="Settings for"
        options={entries.sort(order).map(option)}
        onChange={choose}
      />
    </div>
  );
}
