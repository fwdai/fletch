import type { BackgroundTaskMap } from "@desktop/adapters/shared/backgroundTasks";
import { Icon } from "@desktop/components/Icon";
import { fmtElapsed, useTick } from "../../lib/hooks";
import { quietHint, subagentName, visibleSubagents } from "../../lib/subagents";

/** Sub-agents still working after the turn (or failed lately), pinned above
 *  the transcript so they stay in view while the log scrolls. Tapping one
 *  jumps to the tool row that launched it. */
export function SubagentStrip({
  tasks,
  onJump,
}: {
  tasks: BackgroundTaskMap | undefined;
  onJump: (toolUseId: string) => void;
}) {
  const running = Object.values(tasks ?? {}).some((t) => t.status === "running");
  useTick(1000, running);
  const now = Date.now();
  const list = visibleSubagents(tasks, now);
  if (list.length === 0) return null;
  return (
    <div className="subagents">
      {list.map((t) => {
        const failed = t.status === "failed";
        const quiet = quietHint(t, now);
        return (
          <button
            type="button"
            key={t.taskId}
            className={`subagent${failed ? " err" : ""}`}
            onClick={() => onJump(t.toolUseId)}
          >
            {failed ? (
              <Icon name="alert" size={12} />
            ) : (
              <span className="working-dots">
                <i />
                <i />
                <i />
              </span>
            )}
            <span className="nm">{subagentName(t)}</span>
            <span className="mt">
              {failed
                ? (t.failureStatus ?? "failed")
                : `${t.toolUses} tool${t.toolUses === 1 ? "" : "s"} · ${fmtElapsed(
                    Math.floor((now - t.startedAt) / 1000),
                  )}`}
              {quiet && <span className="quiet"> · {quiet}</span>}
            </span>
          </button>
        );
      })}
    </div>
  );
}
