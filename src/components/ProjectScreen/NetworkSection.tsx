import { useState } from "react";
import {
  api,
  type EgressEntry,
  type ProjectFileEdit,
  type ProjectFileReport,
  type ProjectFileWhich,
} from "@/api";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import type { BlockedHost } from "@/store/sandbox";
import { EGRESS_CATEGORIES, useEgressOverview } from "@/util/useEgressOverview";
import { useProjectFile } from "@/util/useProjectFile";

const FILE_LABEL: Record<ProjectFileWhich, string> = {
  shared: ".fletch/settings.json",
  local: ".fletch/settings.local.json",
};

/** What this project's box agents may reach, and where each part of it comes
 *  from: Fletch's list and the user's global changes (Settings › Sandbox),
 *  then this project's own — `.fletch/settings.json` (shared, meant to be
 *  committed) and `.fletch/settings.local.json` (just you), read on the host
 *  from the project's repository, never from an agent's checkout.
 *
 *  Edits made here are written into one of those files, picked by the user,
 *  and apply at once. Hosts a file adds by any other hand wait for approval:
 *  the one decision a file in a repository cannot make for the user. */
export function NetworkSection({ projectId }: { projectId: string }) {
  const fileGate = useGate("projectFile");
  const editGate = useGate("projectFileEdit");
  const policyGate = useGate("egressPolicy");
  const { status, approve } = useProjectFile(fileGate ? null : projectId);
  const { overview } = useEgressOverview(projectId, !!policyGate);
  const blocked = useAppStore((s) => s.boxEgressBlocked);
  const clearBlocked = useAppStore((s) => s.clearBoxEgress);
  const agents = useAppStore((s) => s.workspace?.agents);
  const projects = useAppStore((s) => s.workspace?.projects);
  const [which, setWhich] = useState<ProjectFileWhich | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const files = status?.files ?? [];
  const sharedExists = files.find((f) => f.path === FILE_LABEL.shared)?.exists ?? false;
  // Shared when the team already keeps a file, else just you.
  const target: ProjectFileWhich = which ?? (sharedExists ? "shared" : "local");
  const pending = status?.egress.pending ?? [];
  const present = files.filter((f) => f.exists);
  const canEdit = !editGate && !!status && !status.error;

  // The workspaces of this project, for its blocked hosts.
  const paths = new Set(
    (projects ?? []).filter((p) => p.project_id === projectId).map((p) => p.path),
  );
  const mine = new Set(
    (agents ?? []).filter((a) => a.repos[0] && paths.has(a.repos[0].repo_path)).map((a) => a.id),
  );
  const allowedNow = new Set(overview?.project?.entries.map((e) => e.pattern) ?? []);
  const recent = byHost(blocked.filter((b) => mine.has(b.agent_id) && !allowedNow.has(b.host)));

  const run = async (task: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await task();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const edit = (file: ProjectFileWhich, change: ProjectFileEdit) =>
    run(() => api.projectFileEdit(projectId, file, change));
  const fileHolding = (pattern: string, list: "allow" | "disable"): ProjectFileWhich | null => {
    for (const w of ["shared", "local"] as const) {
      if (files.find((f) => f.path === FILE_LABEL[w])?.[list].includes(pattern)) return w;
    }
    return null;
  };

  const own = overview?.project?.entries.filter(
    (e) => e.source === "project_file" || e.source === "project",
  );
  const inherited = overview?.base ?? [];

  return (
    <section className="ps-section ps-pf">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Network</h2>
        <p className="ps-section-lead text-sm">
          What this project&rsquo;s box agents may reach: Fletch&rsquo;s list and your changes in
          Settings › Sandbox, then this project&rsquo;s <code>.fletch/settings.json</code> (commit
          it to share with your team) and <code>.fletch/settings.local.json</code> (just you).
        </p>
      </header>

      {fileGate ? (
        <p className="ps-pf-note text-sm">{fileGate}</p>
      ) : !status ? null : status.error ? (
        <p className="ps-pf-error text-sm">{status.error}</p>
      ) : (
        <>
          {present.length > 0 && (
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
                A settings file asks for these. Box agents of this project could reach them. Approve
                only hosts you expect this repository to need.
              </p>
              <div className="ps-pf-actions">
                <Button
                  variant="primary"
                  onClick={() => void run(() => approve(pending))}
                  disabled={busy}
                >
                  Allow {pending.length === 1 ? "this host" : "these hosts"}
                </Button>
              </div>
            </div>
          )}

          {canEdit && (
            <div className="ps-field">
              <span className="ps-label text-sm">Allow a host</span>
              <div className="ps-name-row">
                <input
                  className="ps-input mono text-sm"
                  value={draft}
                  placeholder="static.example.com or *.example.com"
                  spellCheck={false}
                  onChange={(e) => {
                    setError(null);
                    setDraft(e.target.value);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && draft.trim())
                      void edit(target, { allowAdd: [draft.trim()] }).then(() => setDraft(""));
                  }}
                />
                <select
                  className="ps-input ps-pf-which text-sm"
                  value={target}
                  onChange={(e) => setWhich(e.target.value as ProjectFileWhich)}
                  aria-label="Which settings file"
                >
                  <option value="shared">Shared · settings.json</option>
                  <option value="local">Just me · settings.local.json</option>
                </select>
                <Button
                  variant="outline"
                  disabled={busy || !draft.trim()}
                  onClick={() =>
                    void edit(target, { allowAdd: [draft.trim()] }).then(() => setDraft(""))
                  }
                >
                  Add
                </Button>
              </div>
            </div>
          )}
          {error && <p className="ps-pf-error text-sm">{error}</p>}

          {recent.length > 0 && (
            <div className="ps-field">
              <span className="ps-label text-sm">Blocked recently</span>
              <ul className="ps-pf-rows">
                {recent.map((b) => (
                  <li key={b.host} className="ps-pf-row">
                    <span className="mono">{b.host}</span>
                    <span className="ps-pf-ok">
                      {b.count === 1 ? "once" : `${b.count} times`}
                      {b.quiet && " · telemetry"}
                    </span>
                    {canEdit && (
                      <Button
                        variant="ghost"
                        disabled={busy}
                        onClick={() =>
                          void edit(target, { allowAdd: [b.host] }).then(() => clearBlocked(b.host))
                        }
                      >
                        Allow
                      </Button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}

          {own && own.length > 0 && (
            <div className="ps-field">
              <span className="ps-label text-sm">Allowed for this project</span>
              <ul className="ps-pf-rows">
                {own.map((e) => {
                  const file = e.source === "project_file" ? fileHolding(e.pattern, "allow") : null;
                  return (
                    <li key={e.pattern} className="ps-pf-row">
                      <span className="mono">{e.pattern}</span>
                      <span className="ps-pf-ok">
                        {file ? FILE_LABEL[file] : "added in Fletch"}
                      </span>
                      {canEdit && file && (
                        <Button
                          variant="ghost"
                          disabled={busy}
                          onClick={() => void edit(file, { allowRemove: [e.pattern] })}
                        >
                          Remove
                        </Button>
                      )}
                    </li>
                  );
                })}
              </ul>
            </div>
          )}

          {inherited.length > 0 && (
            <details className="ps-pf-inherited">
              <summary className="ps-label text-sm">
                From Fletch and your global list · {inherited.length} hosts
              </summary>
              <p className="ps-pf-note text-sm">
                Turning one off writes it to the chosen settings file for this project only.
              </p>
              {groups(inherited).map(({ label, entries }) => (
                <div key={label} className="ps-pf-group">
                  <div className="ps-pf-group-t text-sm">{label}</div>
                  <ul className="ps-pf-rows">
                    {entries.map((e) => {
                      const on = allowedNow.has(e.pattern);
                      const offIn = on ? null : fileHolding(e.pattern, "disable");
                      return (
                        <li key={e.pattern} className="ps-pf-row">
                          <span className={`mono ${on ? "" : "ps-pf-off"}`}>{e.pattern}</span>
                          {canEdit && (on || offIn) && (
                            <Button
                              variant="ghost"
                              disabled={busy}
                              onClick={() =>
                                void (on
                                  ? edit(target, { disableAdd: [e.pattern] })
                                  : offIn && edit(offIn, { disableRemove: [e.pattern] }))
                              }
                            >
                              {on ? "Turn off" : "Turn on"}
                            </Button>
                          )}
                        </li>
                      );
                    })}
                  </ul>
                </div>
              ))}
            </details>
          )}
        </>
      )}
    </section>
  );
}

/** The inherited list, the template's categories first, then the user's
 *  global additions. */
function groups(entries: EgressEntry[]): { label: string; entries: EgressEntry[] }[] {
  const out = EGRESS_CATEGORIES.map(({ key, label }) => ({
    label,
    entries: entries.filter((e) => e.category === key),
  }));
  out.push({
    label: "Your global list",
    entries: entries.filter((e) => e.source === "global"),
  });
  return out.filter((g) => g.entries.length > 0);
}

/** Blocked hosts across this project's workspaces, one row per host. */
function byHost(blocked: BlockedHost[]) {
  const rows = new Map<string, { host: string; count: number; quiet: boolean }>();
  for (const b of blocked) {
    const row = rows.get(b.host);
    if (row) row.count += b.count;
    else rows.set(b.host, { host: b.host, count: b.count, quiet: b.quiet });
  }
  // What the user may need first: not noise, then the most refused.
  return [...rows.values()].sort((a, b) => Number(a.quiet) - Number(b.quiet) || b.count - a.count);
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
