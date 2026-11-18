import { EnvironmentDot } from "@/components/HostChip";
import { Icon, type IconName } from "@/components/Icon";
import { connectionLabel } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";

interface Props {
  /** The place the composer's agent runs. */
  env: EnvironmentEntry;
  /** This Mac's glyph, when something other than the laptop says more. */
  icon?: IconName;
  /** What the place runs the workspace in, after its name. */
  detail?: string;
  /** Replaces "Runs on <name>". */
  tip?: string;
  /** The menu of places the tab opens. Without one the tab only says where
   *  the agent runs. */
  menu?: { open: boolean; onToggle: () => void };
}

/** Where the composer's agent runs, on the tab at the composer's top edge: a
 *  paired host by its connection dot and name, This Mac by its glyph. */
export function EnvironmentTab({ env, icon = "laptop", detail, tip, menu }: Props) {
  const remote = env.kind === "remote";
  const where = remote ? `Runs on ${env.name} · ${connectionLabel(env)}` : `Runs on ${env.name}`;
  const body = (
    <>
      {remote ? <EnvironmentDot env={env} /> : <Icon name={icon} size={12} />}
      <span className="env-tab-name">{env.name}</span>
      {detail && (
        <>
          <span className="env-tab-sep" aria-hidden>
            ·
          </span>
          <span className="env-tab-detail">{detail}</span>
        </>
      )}
    </>
  );
  if (!menu) {
    return (
      <div className="env-tab tip" data-tip={tip ?? where}>
        {body}
      </div>
    );
  }
  return (
    <button
      type="button"
      className="env-tab is-action tip"
      data-tip={tip ?? where}
      aria-haspopup="menu"
      aria-expanded={menu.open}
      onClick={menu.onToggle}
    >
      {body}
      <Icon name="chevD" size={9} />
    </button>
  );
}
