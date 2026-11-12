import type { ProjectFileWhich } from "@/api";
import { SetSeg } from "@/components/SettingsScreen/primitives";

/** Which settings file an edit goes to. */
export function FileTarget({
  value,
  onChange,
}: {
  value: ProjectFileWhich;
  onChange: (which: ProjectFileWhich) => void;
}) {
  return (
    <SetSeg
      value={value}
      options={[
        {
          value: "shared",
          label: "Shared",
          tip: "Writes .fletch/settings.json. Commit it to share with your team.",
        },
        { value: "local", label: "Just me", tip: "Writes .fletch/settings.local.json." },
      ]}
      onChange={onChange}
    />
  );
}
