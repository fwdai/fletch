import { EnvironmentDot } from "@/components/HostChip";
import { Icon } from "@/components/Icon";
import { connectionLabel } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";

interface Props {
  /** The machine the composer's agent runs on. */
  env: EnvironmentEntry;
  /** The menu of machines the tab opens. Without one the tab only says where
   *  the agent runs. */
  menu?: { open: boolean; onToggle: () => void };
}

/** Which machine the composer's agent runs on, on the sheet behind the
 *  composer's top edge: a paired host by its connection dot and name, This
 *  Mac by its glyph. */
export function EnvironmentTab({ env, menu }: Props) {
  const remote = env.kind === "remote";
  const tip = remote ? `Runs on ${env.name} · ${connectionLabel(env)}` : `Runs on ${env.name}`;
  const body = (
    <>
      {remote ? <EnvironmentDot env={env} /> : <Icon name="laptop" size={12} />}
      <span className="env-tab-name">{env.name}</span>
    </>
  );
  if (!menu) {
    return (
      <div className="env-tab tip" data-tip={tip}>
        {body}
      </div>
    );
  }
  return (
    <button
      type="button"
      className="env-tab is-action tip"
      data-tip={tip}
      aria-haspopup="menu"
      aria-expanded={menu.open}
      onClick={menu.onToggle}
    >
      {body}
      <Icon name="chevD" size={9} />
    </button>
  );
}
