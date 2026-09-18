import { HostGateNote } from "@/components/HostGateNote";
import { Icon } from "@/components/Icon";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { connectionLabel, hostVersionLabel } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";
import { EnvironmentDot } from "./EnvironmentDot";

/** This Mac first — it is the only `local` entry and it is always there — then
 *  the paired hosts by name, so the list doesn't reshuffle as connections come
 *  and go. */
const order = (a: EnvironmentEntry, b: EnvironmentEntry) =>
  a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "local" ? -1 : 1;

/** An entry's second line: how its connection is doing — the thing that decides
 *  whether switching is worth it — and, for a host that has reported one, the
 *  version it is running. What it cannot do is the line below (`HostGateNote`). */
const subLine = (env: EnvironmentEntry) => {
  const version = hostVersionLabel(env);
  const state = connectionLabel(env);
  return version ? `${state} · ${version}` : state;
};

/** The environment list. Selecting one is the whole interaction: the host
 *  picker for spawning is this, and nothing else — a new agent always lands on
 *  the environment the UI is driving.
 *
 *  A host that is offline is still selectable. It shows whatever it was last
 *  showing (or nothing, on a first visit) with its state beside it, because the
 *  alternative — refusing the click — leaves the user unable to see the work
 *  they left running there. */
export function EnvironmentMenu({ onClose }: { onClose: () => void }) {
  const environments = useAppStore((s) => s.environments);
  const activeId = useAppStore((s) => s.activeEnvironmentId);
  const switchEnvironment = useAppStore((s) => s.switchEnvironment);

  const entries = Object.values(environments).sort(order);

  return (
    <>
      <Scrim onClose={onClose} zIndex={290} />
      <div className="env-menu" role="listbox">
        {entries.map((env) => (
          <button
            key={env.id}
            role="option"
            aria-selected={env.id === activeId}
            className={`env-item flex-center ${env.id === activeId ? "active" : ""}`}
            onClick={() => {
              onClose();
              void switchEnvironment(env.id);
            }}
          >
            <EnvironmentDot env={env} />
            <span className="env-item-text">
              <span className="env-item-name text-base">{env.name}</span>
              <span className="env-item-sub text-sm">{subLine(env)}</span>
              <HostGateNote env={env} className="env-item-sub text-sm" />
            </span>
            {env.id === activeId && <Icon name="check" size={13} />}
          </button>
        ))}
      </div>
    </>
  );
}
