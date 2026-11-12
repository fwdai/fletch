import type { ProjectFileReport } from "@/api";
import { SetRow } from "@/components/SettingsScreen/primitives";
import type { ProjectFileEditor } from "./useProjectFileEditor";

/** The settings files' state as rows: why they can't be read, each file there
 *  is with the warnings `warnings` picks from its report, and why the last
 *  edit failed. `quiet` leaves out a file read without one. */
export function FileStates({
  editor: { gate, status, files, error },
  warnings,
  quiet,
}: {
  editor: ProjectFileEditor;
  warnings: (file: ProjectFileReport) => string[];
  quiet?: boolean;
}) {
  const shown = files.filter((f) => f.exists && !(quiet && !f.error && warnings(f).length === 0));
  return (
    <>
      {gate ? (
        <SetRow title="Settings files" sub={gate} />
      ) : status?.error ? (
        <SetRow title="Settings files" sub={<Problem text={status.error} />} />
      ) : (
        shown.map((f) => (
          <SetRow
            key={f.path}
            compact={!f.error && warnings(f).length === 0}
            title={<span className="mono">{f.path}</span>}
            sub={f.error ? <Problem text={f.error} /> : warnings(f).join(" ") || "Read"}
          />
        ))
      )}
      {error && <SetRow title="Couldn't save" sub={<Problem text={error} />} />}
    </>
  );
}

function Problem({ text }: { text: string }) {
  return <span className="ps-pf-error">{text}</span>;
}
