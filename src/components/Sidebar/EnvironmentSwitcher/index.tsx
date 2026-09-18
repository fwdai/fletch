import { useState } from "react";
import { Icon } from "@/components/Icon";
import { useAppStore } from "@/store";
import { EnvironmentDot } from "./EnvironmentDot";
import { EnvironmentMenu } from "./EnvironmentMenu";

/** Which engine the sidebar — and everything hanging off it — is showing:
 *  This Mac, or one of the paired hosts (docs/multi-host-plan.md §5.2).
 *
 *  Renders NOTHING when there are no paired hosts, which is the overwhelmingly
 *  common case and the one the plan's hard constraint is about: with no hosts
 *  saved, the sidebar header is exactly what it was. The switcher appears the
 *  moment a host is paired in Settings › Remote control and disappears again
 *  when the last one is forgotten. */
export function EnvironmentSwitcher() {
  const [open, setOpen] = useState(false);
  // A count, not the key list: a selector that builds a fresh array would make
  // this re-render on every unrelated store write.
  const count = useAppStore((s) => Object.keys(s.environments).length);
  const active = useAppStore((s) => s.environments[s.activeEnvironmentId]);

  if (count < 2 || !active) return null;

  return (
    <div className="env-switch">
      <button
        className="env-btn flex-center text-sm"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <EnvironmentDot env={active} />
        <span className="env-btn-name">{active.name}</span>
        <Icon name="chevD" size={12} />
      </button>
      {open && <EnvironmentMenu onClose={() => setOpen(false)} />}
    </div>
  );
}
