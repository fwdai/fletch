import { useAppStore } from "@/store";
import { Icon } from "./Icon";
import { ProjectFilePrompt, useProjectFilePrompts } from "./ProjectFilePrompts";
import { Button } from "./ui/Button";

/**
 * Bottom-left toasts for connections a box's egress proxy refused, one per
 * denial, each dismissed by hand. Fed by `box:egress` (see
 * store/eventListeners), which the box engine emits for every answer; only the
 * denied ones land here. Allowing a host from this toast is not built yet, so
 * for now it only says what was blocked. Above them sit the prompts of a
 * project's `.fletch/settings.json` asking for hosts the user has not approved
 * (`ProjectFilePrompts`): the same stack, since both are about what a box may
 * reach. Renders nothing when there is nothing to show, which is nearly always.
 */
export function BoxEgressToast() {
  const denials = useAppStore((s) => s.boxEgressDenials);
  const dismiss = useAppStore((s) => s.dismissBoxEgress);
  const agents = useAppStore((s) => s.workspace?.agents);
  const { prompts, dismiss: dismissPrompt } = useProjectFilePrompts();

  if (denials.length === 0 && prompts.length === 0) return null;

  return (
    <div className="docker-build-stack">
      {prompts.map((status) => (
        <ProjectFilePrompt
          key={status.project_id}
          status={status}
          onDismiss={() => dismissPrompt(status)}
        />
      ))}
      {denials.map((d, i) => {
        const name = agents?.find((a) => a.id === d.agent_id)?.name ?? d.agent_id;
        return (
          <div
            // A denial has no id of its own; the index is its position in the
            // dismissable list, and dismissing one shifts the rest.
            key={`${d.agent_id}:${d.host}:${d.port}:${i}`}
            className="update-toast docker-build-toast"
            role="alert"
          >
            <Icon name="close" />
            <div className="update-toast-body">
              <div className="update-toast-text">
                <strong>
                  Blocked connection to {d.host}:{d.port}
                </strong>
                <span>{name} runs in a box, which reaches only the project's allowed hosts.</span>
              </div>
              <div className="update-toast-actions">
                <Button variant="ghost" onClick={() => dismiss(i)}>
                  Dismiss
                </Button>
              </div>
            </div>
          </div>
        );
      })}
    </div>
  );
}
