import { PickerSheet, ProjectIcon } from "../components/ui";
import { OnHost } from "../host/OnHost";
import { type ProjectEntry, primaryPath } from "../lib/projects";

/** Every paired host's projects, one row each, as its speaking host has it. */
export function ProjectPickerSheet({
  open,
  onClose,
  entries,
  value,
  onPick,
}: {
  open: boolean;
  onClose: () => void;
  entries: ProjectEntry[];
  value: ProjectEntry | undefined;
  onPick: (entry: ProjectEntry) => void;
}) {
  return (
    <PickerSheet
      open={open}
      onClose={onClose}
      title="Project"
      items={entries.map((e) => ({
        id: e.key,
        label: e.project.name,
        sub: primaryPath(e.project),
        icon: (
          <OnHost hostKey={e.hostKey}>
            <ProjectIcon project={e.project} />
          </OnHost>
        ),
      }))}
      value={value?.key ?? null}
      onChange={(key) => {
        const picked = entries.find((e) => e.key === key);
        if (picked) onPick(picked);
      }}
    />
  );
}
