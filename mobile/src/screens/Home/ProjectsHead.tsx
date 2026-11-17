import { Icon } from "@desktop/components/Icon";
import { isBusy } from "../../lib/agents";
import { type HostState, useEveryHost, useHostNav } from "../../store";

const workspaceOf = (s: HostState) => s.workspace;
const pendingOf = (s: HostState) =>
  Object.values(s.pendingToolUse).reduce((n, m) => n + Object.keys(m).length, 0);

/** The line over Home's projects: how many there are, a way to add one while
 *  a host is up (`canAdd`), and how many agents on every host need the user or
 *  are working. */
export function ProjectsHead({ count, canAdd }: { count: number; canAdd: boolean }) {
  const { openSheet } = useHostNav();
  const workspaces = useEveryHost(workspaceOf);
  const pending = useEveryHost(pendingOf).reduce((n, p) => n + p, 0);
  const agents = workspaces.flatMap((w) => w?.agents ?? []);
  const errored = agents.filter((a) => a.status === "error").length;
  const running = agents.filter(isBusy).length;
  const attention = pending + errored;
  return (
    <div className="sect">
      Projects <span className="n">{count}</span>
      {canAdd && (
        <button type="button" className="sect-add" onClick={() => openSheet("addProject")}>
          <Icon name="plus" size={12} strokeWidth={2.2} />
          Add
        </button>
      )}
      <span className="grow" />
      {attention > 0 && (
        <span
          className="n"
          style={{
            color: "var(--warn)",
            textTransform: "none",
            letterSpacing: 0,
            display: "inline-flex",
            alignItems: "center",
            gap: 4,
          }}
        >
          <Icon name="hand" size={11} strokeWidth={1.8} />
          {attention}
        </span>
      )}
      {running > 0 && (
        <span
          className="n"
          style={{ color: "var(--success)", textTransform: "none", letterSpacing: 0 }}
        >
          {running} running
        </span>
      )}
    </div>
  );
}
