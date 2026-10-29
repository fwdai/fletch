import { useEffect, useMemo, useState } from "react";
import { api, onProjectFileChanged, type ProjectFileStatus } from "@/api";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { Icon } from "./Icon";
import { Button } from "./ui/Button";

/** What makes a prompt the same prompt: its project and the hosts it waits on.
 *  A dismissed prompt comes back only when that set changes. */
const promptKey = (s: ProjectFileStatus) => `${s.project_id}\n${s.egress.pending.join("\n")}`;

/** Every project's `.fletch/settings.json` status whose file adds hosts the
 *  user has not approved yet, minus the prompts dismissed for this session.
 *  Read once per project on load and again on `project_file:changed`. */
export function useProjectFilePrompts(): {
  prompts: ProjectFileStatus[];
  dismiss: (s: ProjectFileStatus) => void;
} {
  const projects = useAppStore((s) => s.workspace?.projects);
  const environmentId = useAppStore((s) => s.activeEnvironmentId);
  const gate = useGate("projectFile");
  const [statuses, setStatuses] = useState<Record<string, ProjectFileStatus>>({});
  const [dismissed, setDismissed] = useState<Set<string>>(() => new Set());

  // One entry per project, however many repos it has.
  const idsKey = useMemo(
    () => [...new Set((projects ?? []).map((p) => p.project_id))].sort().join(","),
    [projects],
  );

  // biome-ignore lint/correctness/useExhaustiveDependencies: environmentId is the intended re-run trigger, not an unused dep
  useEffect(() => {
    setStatuses({});
    if (gate || !idsKey) return;
    let live = true;
    const read = (projectId: string) =>
      api
        .projectFileStatus(projectId)
        .then((s) => live && setStatuses((prev) => ({ ...prev, [projectId]: s })))
        .catch(() => {});
    for (const id of idsKey.split(",")) read(id);
    const unlisten = onProjectFileChanged((e) => read(e.project_id));
    return () => {
      live = false;
      void unlisten.then((off) => off());
    };
  }, [idsKey, environmentId, gate]);

  const prompts = Object.values(statuses).filter(
    (s) => s.egress.pending.length > 0 && !dismissed.has(promptKey(s)),
  );
  const dismiss = (s: ProjectFileStatus) => setDismissed((prev) => new Set(prev).add(promptKey(s)));
  return { prompts, dismiss };
}

/** One project's settings file asks for hosts the user has not approved: say
 *  which project and how many, and lead to the project page where they are
 *  listed and approved. Rendered in the box network toast stack. */
export function ProjectFilePrompt({
  status,
  onDismiss,
}: {
  status: ProjectFileStatus;
  onDismiss: () => void;
}) {
  const projects = useAppStore((s) => s.workspace?.projects);
  const openProjectScreen = useAppStore((s) => s.openProjectScreen);
  const project = projects?.find((p) => p.project_id === status.project_id);
  const name = project?.name ?? "A project";
  const path = status.repo_path ?? project?.path;
  const n = status.egress.pending.length;

  return (
    <div className="update-toast docker-build-toast" role="alert">
      <Icon name="cube" />
      <div className="update-toast-body">
        <div className="update-toast-text">
          <strong>
            {name}&rsquo;s settings file asks to allow {n === 1 ? "a host" : `${n} hosts`}
          </strong>
          <span>
            Its .fletch/settings.json would let box agents reach{" "}
            {n === 1 ? status.egress.pending[0] : "more hosts"}. Review before they apply.
          </span>
        </div>
        <div className="update-toast-actions">
          {path && (
            <Button
              variant="primary"
              onClick={() => {
                openProjectScreen(path, "settings");
                onDismiss();
              }}
            >
              Review
            </Button>
          )}
          <Button variant="ghost" onClick={onDismiss}>
            Later
          </Button>
        </div>
      </div>
    </div>
  );
}
