import { EnvironmentDot } from "@/components/HostChip";
import { Icon } from "@/components/Icon";
import { connectionLabel } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";
import { type WorkspaceTypeChip, workspaceTypeIcon, workspaceTypeLabel } from "../workspaceType";
import type { RunTargetOption } from "./options";

interface Props {
  /** The place picked now. */
  option: RunTargetOption;
  env: EnvironmentEntry | undefined;
  /** This Mac's workspace types, while it is the place picked and the draft
   *  is This Mac's. */
  types: WorkspaceTypeChip;
  open: boolean;
  onClick: () => void;
}

/** What the tooltip says: why the place can't run the workspace, else where
 *  it runs. */
function tipFor(option: RunTargetOption, env: EnvironmentEntry | undefined, types: Props["types"]) {
  if (option.reason) return option.reason;
  if (types) return types.tip;
  return env?.kind === "remote"
    ? `Runs on ${option.label} · ${connectionLabel(env)}`
    : `Runs on ${option.label}`;
}

/** The pill for where the workspace runs: a paired host by its dot and name,
 *  This Mac with the workspace type it runs in once that is known. */
export function TargetPill({ option, env, types, open, onClick }: Props) {
  const shown = types?.kind === "menu" ? types.shown : null;
  return (
    <button
      type="button"
      className="pill is-action tip"
      data-tip={tipFor(option, env, types)}
      aria-haspopup="menu"
      aria-expanded={open}
      onClick={onClick}
    >
      {env?.kind === "remote" ? (
        <EnvironmentDot env={env} />
      ) : (
        <Icon
          name={
            shown ? workspaceTypeIcon(shown.kind) : types?.kind === "retry" ? "refresh" : "laptop"
          }
        />
      )}
      <span style={{ color: "var(--fg-2)" }}>on</span>
      <span className="v">{option.label}</span>
      {shown && (
        <>
          <span style={{ color: "var(--fg-2)" }}>in</span>
          <span className="v">{workspaceTypeLabel(shown.kind)}</span>
        </>
      )}
      <Icon name="chevD" size={9} />
    </button>
  );
}
