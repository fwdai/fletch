import type { EgressAnswer, EgressPrompt } from "@/api";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { Icon } from "./Icon";
import { Button } from "./ui/Button";

/** One workspace's VM is holding a connection until the user decides (the Ask
 *  me posture). One card per workspace however many hosts wait, showing the
 *  oldest; answering it brings up the next. Each answer is about exactly this
 *  host: allow it for the project (remembered in the project's list), for
 *  every project (the global list), or block it on this VM until it restarts.
 *  "Open for this turn" is the fourth way out, for a turn that will reach
 *  many hosts. The host refuses an unanswered hold after about a minute, so
 *  there is no dismiss: nothing is lost by leaving it. */
export function BoxEgressPrompt({
  agentId,
  prompts,
}: {
  agentId: string;
  prompts: EgressPrompt[];
}) {
  const agents = useAppStore((s) => s.workspace?.agents);
  const answer = useAppStore((s) => s.answerEgressPrompt);
  const answerGate = useGate("egressPrompt");
  const turnGate = useGate("egressTurn");
  const head = prompts[0];
  if (!head) return null;
  const name = agents?.find((a) => a.id === agentId)?.name ?? "A workspace";
  const more = prompts.length - 1;
  const reply = (how: EgressAnswer) => void answer(head.id, how);

  return (
    <div className="update-toast docker-build-toast box-egress-prompt" role="alertdialog">
      <Icon name="globe" />
      <div className="update-toast-body">
        <div className="update-toast-text">
          <strong>
            {name} wants to reach <span className="mono">{head.host}</span>
          </strong>
          <span>
            {answerGate ??
              (more > 0
                ? `${more} more ${more === 1 ? "host is" : "hosts are"} waiting. Unanswered, it's blocked in a minute.`
                : "Allowed hosts are remembered. Unanswered, it's blocked in a minute.")}
          </span>
        </div>
        {!answerGate && (
          <div className="update-toast-actions">
            <Button variant="primary" onClick={() => reply("project")}>
              This project
            </Button>
            <Button variant="outline" onClick={() => reply("always")}>
              All projects
            </Button>
            <Button variant="ghost" onClick={() => reply("block")}>
              Block
            </Button>
            {!turnGate && (
              <Button
                variant="ghost"
                tip="Any host passes until this turn ends, then the posture resumes"
                onClick={() => reply("turn")}
              >
                Open this turn
              </Button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

/** The queue grouped by workspace, in the order each one first asked. */
export function promptsByAgent(prompts: EgressPrompt[]): Map<string, EgressPrompt[]> {
  const byAgent = new Map<string, EgressPrompt[]>();
  for (const p of prompts) byAgent.set(p.agent_id, [...(byAgent.get(p.agent_id) ?? []), p]);
  return byAgent;
}
