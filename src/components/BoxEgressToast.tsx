import { useAppStore } from "@/store";
import { BoxEgressPrompt, promptsByAgent } from "./BoxEgressPrompt";
import { Icon } from "./Icon";
import { ProjectFilePrompt, useProjectFilePrompts } from "./ProjectFilePrompts";
import { Button } from "./ui/Button";

/**
 * Bottom-left toasts for what a box may reach. A workspace's box being refused
 * a host it has not been refused before says so once — one toast per
 * workspace, however many hosts and connections — and leads to the project's
 * Network section, where blocked hosts are reviewed and allowed. Known noise
 * (telemetry and the like) never prompts; it is only listed there, and neither
 * does a host the user was already asked about. Fed by `box:egress` through
 * `foldBlocked` (store/sandbox).
 *
 * Above them sit the questions of the Ask me posture (`BoxEgressPrompt`): a
 * VM holding a connection until the user decides, one card per workspace. And
 * above those the prompts of a project's `.fletch/settings.json` asking for
 * hosts the user has not approved (`ProjectFilePrompts`): the same stack,
 * since all three are about what a box may reach. Renders nothing when there
 * is nothing to show, which is nearly always.
 */
export function BoxEgressToast() {
  const blocked = useAppStore((s) => s.boxEgressBlocked);
  const dismiss = useAppStore((s) => s.dismissBoxEgress);
  const agents = useAppStore((s) => s.workspace?.agents);
  const openProjectScreen = useAppStore((s) => s.openProjectScreen);
  const pendingPrompts = useAppStore((s) => s.pendingEgressPrompts);
  const { prompts, dismiss: dismissPrompt } = useProjectFilePrompts();

  // Unseen hosts, grouped by workspace in the order they were first refused.
  const byAgent = new Map<string, string[]>();
  for (const b of blocked) {
    if (b.seen) continue;
    byAgent.set(b.agent_id, [...(byAgent.get(b.agent_id) ?? []), b.host]);
  }
  const held = promptsByAgent(pendingPrompts);

  if (byAgent.size === 0 && prompts.length === 0 && held.size === 0) return null;

  return (
    <div className="docker-build-stack">
      {prompts.map((status) => (
        <ProjectFilePrompt
          key={status.project_id}
          status={status}
          onDismiss={() => dismissPrompt(status)}
        />
      ))}
      {[...held].map(([agentId, waiting]) => (
        <BoxEgressPrompt key={`held:${agentId}`} agentId={agentId} prompts={waiting} />
      ))}
      {[...byAgent].map(([agentId, hosts]) => {
        const agent = agents?.find((a) => a.id === agentId);
        const name = agent?.name ?? agentId;
        const repoPath = agent?.repos[0]?.repo_path;
        const more = hosts.length - 1;
        return (
          <div key={agentId} className="update-toast docker-build-toast" role="alert">
            <Icon name="close" />
            <div className="update-toast-body">
              <div className="update-toast-text">
                <strong>
                  {name} couldn&rsquo;t reach {hosts[0]}
                  {more > 0 && ` and ${more} more`}
                </strong>
                <span>Its box reaches only the hosts allowed for the project.</span>
              </div>
              <div className="update-toast-actions">
                {repoPath && (
                  <Button
                    variant="primary"
                    onClick={() => {
                      openProjectScreen(repoPath, "settings", "network");
                      dismiss(agentId);
                    }}
                  >
                    Review
                  </Button>
                )}
                <Button variant="ghost" onClick={() => dismiss(agentId)}>
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
