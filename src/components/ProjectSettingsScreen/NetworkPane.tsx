import { SetGroup, SetHead, SetRow } from "@/components/SettingsScreen/primitives";
import { FletchAllows } from "@/components/SettingsScreen/Sandbox/Network/FletchAllows";
import type { HostList } from "@/components/SettingsScreen/Sandbox/Network/hostList";
import { PostureRow } from "@/components/SettingsScreen/Sandbox/Network/PostureRow";
import { RecentHosts } from "@/components/SettingsScreen/Sandbox/Network/RecentHosts";
import { YourHosts } from "@/components/SettingsScreen/Sandbox/Network/YourHosts";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { useEgressOverview } from "@/util/useEgressOverview";
import { FileStates } from "./projectFile/FileStates";
import { FileTarget } from "./projectFile/FileTarget";
import { FILE_LABEL, fileHolding } from "./projectFile/files";
import { useProjectFileEditor } from "./projectFile/useProjectFileEditor";

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
  const editor = useProjectFileEditor(projectId);
  const { status, files, approve, target, setTarget, canEdit, busy, run, edit } = editor;
  const policyGate = useGate("egressPolicy");
  // A host from before the egress log keeps no record; what this window saw
  // stands in for it below.
  const activityGate = useGate("egressActivity");
  const { overview } = useEgressOverview(projectId, !!policyGate);
  const clearBlocked = useAppStore((s) => s.clearBoxEgress);

  const pending = status?.egress.pending ?? [];
  const entries = overview?.project?.entries ?? [];
  const allowedNow = new Set(entries.map((e) => e.pattern));
  const holding = (pattern: string, list: "allow" | "disable") =>
    fileHolding(files, (f) => f[list].includes(pattern));

  const own: HostList = {
    projectId,
    hosts: entries
      .filter((e) => e.source === "project_file" || e.source === "project")
      .map(({ pattern, source }) => {
        const file = source === "project_file" ? holding(pattern, "allow") : null;
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
    const offIn = on ? null : holding(pattern, "disable");
    const flip = on
      ? () => void edit(target, { disableAdd: [pattern] })
      : offIn
        ? () => void edit(offIn, { disableRemove: [pattern] })
        : undefined;
    return { on, flip: canEdit ? flip : undefined };
  };

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

      <YourHosts project={own} target={<FileTarget value={target} onChange={setTarget} />}>
        <FileStates editor={editor} warnings={(f) => f.warnings} />
      </YourHosts>

      {activityGate ? (
        <BlockedWithoutLog projectId={projectId} except={allowedNow} allow={own.allow} />
      ) : (
        <RecentHosts project={own} />
      )}
      {overview && <FletchAllows overview={overview} host={inherited} />}
    </div>
  );
}

/** The project's refused hosts on a host from before the egress log, from what
 *  this window saw (`boxEgressBlocked`), so the egress toast's Review still
 *  lands on the host it names. Quiet rows — telemetry — stay out, as they never
 *  prompt; allowing one clears it from the toast's list too. */
function BlockedWithoutLog({
  projectId,
  except,
  allow,
}: {
  projectId: string;
  /** Hosts already on the project's list. */
  except: Set<string>;
  allow?: (host: string) => Promise<unknown>;
}) {
  const blocked = useAppStore((s) => s.boxEgressBlocked);
  const agents = useAppStore((s) => s.workspace?.agents);
  const projects = useAppStore((s) => s.workspace?.projects);
  const paths = new Set(
    (projects ?? []).filter((p) => p.project_id === projectId).map((p) => p.path),
  );
  const mine = new Set(
    (agents ?? []).filter((a) => a.repos[0] && paths.has(a.repos[0].repo_path)).map((a) => a.id),
  );
  const counts = new Map<string, number>();
  for (const b of blocked) {
    if (b.quiet || !mine.has(b.agent_id) || except.has(b.host)) continue;
    counts.set(b.host, (counts.get(b.host) ?? 0) + b.count);
  }
  if (counts.size === 0) return null;
  return (
    <SetGroup label="Blocked recently">
      {[...counts]
        .sort((a, b) => b[1] - a[1])
        .map(([host, count]) => (
          <SetRow
            key={host}
            compact
            title={<span className="mono">{host}</span>}
            sub={count === 1 ? "refused once" : `refused ${count} times`}
          >
            {allow && (
              <Button variant="ghost" onClick={() => void allow(host).catch(() => {})}>
                Allow
              </Button>
            )}
          </SetRow>
        ))}
    </SetGroup>
  );
}
