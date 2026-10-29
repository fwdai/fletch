import { useState } from "react";
import type { ProjectFileReport } from "@/api";
import { Button } from "@/components/ui/Button";
import { useGate } from "@/store/capabilities";
import { useProjectFile } from "@/util/useProjectFile";

/** The project's own settings file, `.fletch/settings.json` (shared, meant to
 *  be committed) and `.fletch/settings.local.json` (just you), read on the
 *  host from the project's repository — never from an agent's checkout. Shows
 *  what each file says, and holds the one decision a file cannot make for the
 *  user: hosts it adds for box agents wait here until approved. */
export function ProjectFileSection({ projectId }: { projectId: string }) {
  const gate = useGate("projectFile");
  const { status, approve } = useProjectFile(gate ? null : projectId);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pending = status?.egress.pending ?? [];
  const applied = status?.egress.applied ?? [];
  const present = status?.files.filter((f) => f.exists) ?? [];

  const allow = async () => {
    setBusy(true);
    setError(null);
    try {
      await approve(pending);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="ps-section ps-pf">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Settings file</h2>
        <p className="ps-section-lead text-sm">
          Fletch reads <code>.fletch/settings.json</code> (commit it to share with your team) and{" "}
          <code>.fletch/settings.local.json</code> (just you) from this project&rsquo;s repository.
          Hosts they let box agents reach apply once you approve them.
        </p>
      </header>

      {gate ? (
        <p className="ps-pf-note text-sm">{gate}</p>
      ) : !status ? null : status.error ? (
        <p className="ps-pf-error text-sm">{status.error}</p>
      ) : (
        <>
          {present.length === 0 ? (
            <p className="ps-pf-note text-sm">
              No settings file yet. To let this project&rsquo;s boxes reach a host, add{" "}
              <code>{'{ "egress": { "allow": ["static.rust-lang.org"] } }'}</code>.
            </p>
          ) : (
            <ul className="ps-pf-files">
              {present.map((file) => (
                <FileRow key={file.path} file={file} />
              ))}
            </ul>
          )}

          {pending.length > 0 && (
            <div className="ps-pf-pending" role="alert">
              <div className="ps-pf-pending-t text-sm">
                {pending.length === 1
                  ? "1 host is waiting for your approval"
                  : `${pending.length} hosts are waiting for your approval`}
              </div>
              <ul className="ps-pf-hosts mono">
                {pending.map((host) => (
                  <li key={host}>{host}</li>
                ))}
              </ul>
              <p className="ps-pf-note text-sm">
                Box agents of this project could reach these hosts. Approve only hosts you expect
                this repository to need.
              </p>
              <div className="ps-pf-actions">
                <Button variant="primary" onClick={allow} disabled={busy}>
                  Allow {pending.length === 1 ? "this host" : "these hosts"}
                </Button>
              </div>
              {error && <p className="ps-pf-error text-sm">{error}</p>}
            </div>
          )}

          {applied.length > 0 && (
            <div className="ps-field">
              <span className="ps-label text-sm">Allowed from the file</span>
              <ul className="ps-pf-hosts mono">
                {applied.map((host) => (
                  <li key={host}>{host}</li>
                ))}
              </ul>
            </div>
          )}
        </>
      )}
    </section>
  );
}

function FileRow({ file }: { file: ProjectFileReport }) {
  return (
    <li className="ps-pf-file">
      <span className="mono">{file.path}</span>
      {file.error ? (
        <span className="ps-pf-error">{file.error}</span>
      ) : (
        <span className="ps-pf-ok">read</span>
      )}
      {file.warnings.map((w) => (
        <span key={w} className="ps-pf-warn">
          {w}
        </span>
      ))}
    </li>
  );
}
