import { Icon } from "@/components/Icon";
import { MenuButton } from "@/components/ui/MenuButton";
import type { ProjectFocus } from "../projectFocus";
import { SIGNAL_LABEL, strongestSignal } from "../projectSignal";
import type { ProjectOption } from "./ProjectOptionRow";
import { ProjectSelectorMenu } from "./ProjectSelectorMenu";
import { SignalDot } from "./SignalDot";

export type { ProjectOption };

interface Props {
  projects: readonly ProjectOption[];
  focus: ProjectFocus;
  onChange: (focus: string[]) => void;
}

export function ProjectSelector({ projects, focus, onChange }: Props) {
  const narrowed = focus.length > 0;
  const hiddenSignal = narrowed
    ? strongestSignal(projects.filter((p) => !focus.includes(p.key)).map((p) => p.signal))
    : null;
  const shown = narrowed ? `${focus.length} of ${projects.length}` : "all";
  const scope = narrowed ? `${shown} projects` : "All projects";
  const hidden = hiddenSignal && `${SIGNAL_LABEL[hiddenSignal]} in a hidden project`;

  return (
    <MenuButton
      anchorClassName="proj-select"
      trigger={(trigger) => (
        <button
          type="button"
          className={`side-scope flex-center tip ${narrowed ? "on" : ""}`}
          data-tip={hidden ? `${scope}\n${hidden}` : scope}
          aria-label={`Projects shown: ${shown}${hidden ? `. ${hidden}` : ""}`}
          {...trigger}
        >
          <span className="side-scope-icon iflex-center">
            <Icon name="layers" size={13} />
            {hiddenSignal && <SignalDot signal={hiddenSignal} decorative />}
          </span>
          <Icon name="chevD" size={11} className="side-scope-chev" />
        </button>
      )}
    >
      {(close) => (
        <ProjectSelectorMenu projects={projects} focus={focus} onChange={onChange} close={close} />
      )}
    </MenuButton>
  );
}
