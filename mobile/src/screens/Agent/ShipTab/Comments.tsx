import type { AgentRecord } from "@desktop/api/types/agent";
import type { PrComment } from "@desktop/api/types/pr";
import { Icon } from "@desktop/components/Icon";
import { commentLocation, formatCommentForChat } from "@desktop/components/RightPanel/prComments";
import { htmlToMarkdown, markdownToText } from "@desktop/util/markdownText";
import { isAgentBusy } from "../../../lib/agents";
import { ignore } from "../../../lib/ignore";
import { useStore } from "../../../store";

const EXCERPT = 160;

/** One line of plain prose from a comment body, cut to fit the row. */
export function excerpt(body: string): string {
  const text = markdownToText(htmlToMarkdown(body));
  return text.length > EXCERPT ? `${text.slice(0, EXCERPT).trimEnd()}…` : text;
}

/** Bots first, as the desktop lists them: their comments are already phrased
 *  for an agent, so they are the ones most worth a one-tap hand-off. */
export const botsFirst = (threads: PrComment[]) =>
  [...threads].sort((a, b) => Number(b.is_bot) - Number(a.is_bot));

/** The unresolved review threads, unfolded under the Review evidence row: who
 *  said what, a hand-off to the agent, and the thread on GitHub. */
export function Comments({
  agent,
  threads,
  onDelegated,
}: {
  agent: AgentRecord;
  threads: PrComment[];
  /** Called once a comment has been sent to the agent. */
  onDelegated?: () => void;
}) {
  const send = useStore((s) => s.send);
  const busy = useStore((s) => isAgentBusy(s, agent));
  return (
    <div className="ship-comments">
      {botsFirst(threads).map((c) => {
        const loc = commentLocation(c);
        return (
          <div key={c.id} className="ship-comment">
            <div className="who">
              <b>{c.author}</b>
              {c.is_bot && <span className="pill">bot</span>}
              {loc && <span className="mono loc">{loc}</span>}
            </div>
            <div className="body">{excerpt(c.body)}</div>
            <div className="acts">
              <button
                type="button"
                className="alt"
                disabled={busy}
                onClick={() =>
                  void send(agent.id, formatCommentForChat(c))
                    .then(() => onDelegated?.())
                    .catch(ignore)
                }
              >
                {busy ? "Agent is busy…" : "Send to agent"}
              </button>
              <a href={c.url} target="_blank" rel="noreferrer" aria-label="Open on GitHub">
                <Icon name="external" size={13} />
              </a>
            </div>
          </div>
        );
      })}
    </div>
  );
}
