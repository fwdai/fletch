import { useState } from "react";
import { api, type ProjectFileEdit, type ProjectFileWhich } from "@/api";
import { SetGroup, SetHead, SetRow, SetSeg } from "@/components/SettingsScreen/primitives";
import { FletchAllows } from "@/components/SettingsScreen/Sandbox/Network/FletchAllows";
import type { HostList } from "@/components/SettingsScreen/Sandbox/Network/hostList";
import { PostureRow } from "@/components/SettingsScreen/Sandbox/Network/PostureRow";
import { RecentHosts } from "@/components/SettingsScreen/Sandbox/Network/RecentHosts";
import { YourHosts } from "@/components/SettingsScreen/Sandbox/Network/YourHosts";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { useEgressOverview } from "@/util/useEgressOverview";
import { useProjectFile } from "@/util/useProjectFile";

const FILE_LABEL: Record<ProjectFileWhich, string> = {
  shared: ".fletch/settings.json",
  local: ".fletch/settings.local.json",
};

/** What this project's VM agents may reach, built from the global Network
 *  pane's rows: the posture over the app's, the project's own hosts, what its
 *  VMs reached and were refused lately, and what it inherits from Fletch and
 *  the user's global list. Its own hosts live in `.fletch/settings.json`
 *  (shared, meant to be committed) and `.fletch/settings.local.json` (just
 *  you), read on the host from the project's repository, never from an
 *  agent's checkout, plus the hosts answered "This project" in the prompt,
 *  kept in the app.
 *
 *  Edits made here are written into one of those files, picked by the user,
 *  and apply at once. Hosts a file adds by any other hand wait for approval:
 *  the one decision a file in a repository cannot make for the user. */
export function NetworkPane({ projectId }: { projectId: string }) {
  const fileGate = useGate("projectFile");
  const editGate = useGate("projectFileEdit");
  const policyGate = useGate("egressPolicy");
  const { status, approve } = useProjectFile(fileGate ? null : projectId);
  const { overview } = useEgressOverview(projectId, !!policyGate);
  const clearBlocked = useAppStore((s) => s.clearBoxEgress);
  const [which, setWhich] = useState<ProjectFileWhich | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const files = status?.files ?? [];
  // Shared when the team already keeps a file, else just you.
  const target: ProjectFileWhich =
    which ?? (files.find((f) => f.path === FILE_LABEL.shared)?.exists ? "shared" : "local");
  const pending = status?.egress.pending ?? [];
  const canEdit = !editGate && !!status && !status.error;
  const entries = overview?.project?.entries ?? [];
  const allowedNow = new Set(entries.map((e) => e.pattern));

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
  const fileHolding = (pattern: string, list: "allow" | "disable") =>
    (["shared", "local"] as const).find((w) =>
      files.find((f) => f.path === FILE_LABEL[w])?.[list].includes(pattern),
    ) ?? null;

  const own: HostList = {
    projectId,
    hosts: entries
      .filter((e) => e.source === "project_file" || e.source === "project")
      .map(({ pattern, source }) => {
        const file = source === "project_file" ? fileHolding(pattern, "allow") : null;
        return {
          pattern,
          where: file ? FILE_LABEL[file] : "added in Fletch",
          remove: canEdit && file ? () => edit(file, { allowRemove: [pattern] }) : undefined,
        };
      }),
    // A host allowed here is no longer one the egress toast should hold.
    allow: canEdit
      ? (host) => edit(target, { allowAdd: [host] }).then(() => clearBlocked(host))
      : undefined,
  };
  // An inherited host is on unless a file turns it off; the switch writes the
  // chosen file, or takes the entry out of the file that holds it.
  const inherited = (pattern: string) => {
    const on = allowedNow.has(pattern);
    const offIn = on ? null : fileHolding(pattern, "disable");
    const flip = on
      ? () => void edit(target, { disableAdd: [pattern] })
      : offIn
        ? () => void edit(offIn, { disableRemove: [pattern] })
        : undefined;
    return { on, flip: canEdit ? flip : undefined };
  };
  const problem = (text: string) => <span className="ps-pf-error">{text}</span>;

  return (
    <div className="set-pane">
      <SetHead
        eyebrow="Project settings · Network"
        title="Network"
        desc="What this project's VM agents may reach."
      />
      <PostureRow projectId={projectId} />

      {pending.length > 0 && (
        <SetGroup label="Needs approval">
          <div className="ps-pf-pending" role="alert">
            <SetRow
              title={<span className="mono">{pending.join(" · ")}</span>}
              sub="A settings file in this repository asks for these. Allow only hosts you expect it to need."
            >
              <Button
                variant="primary"
                disabled={busy}
                onClick={() => void run(() => approve(pending))}
              >
                Allow
              </Button>
            </SetRow>
          </div>
        </SetGroup>
      )}

      <YourHosts
        project={own}
        target={
          <SetSeg
            value={target}
            options={[
              {
                value: "shared",
                label: "Shared",
                tip: "Writes .fletch/settings.json. Commit it to share with your team.",
              },
              { value: "local", label: "Just me", tip: "Writes .fletch/settings.local.json." },
            ]}
            onChange={setWhich}
          />
        }
      >
        {fileGate ? (
          <SetRow title="Settings files" sub={fileGate} />
        ) : status?.error ? (
          <SetRow title="Settings files" sub={problem(status.error)} />
        ) : (
          files
            .filter((f) => f.exists)
            .map((f) => (
              <SetRow
                key={f.path}
                compact={!f.error && f.warnings.length === 0}
                title={<span className="mono">{f.path}</span>}
                sub={f.error ? problem(f.error) : f.warnings.join(" ") || "Read"}
              />
            ))
        )}
        {error && <SetRow title="Couldn't save" sub={problem(error)} />}
      </YourHosts>

      <RecentHosts project={own} />
      {overview && <FletchAllows overview={overview} host={inherited} />}
    </div>
  );
}
