import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import { useEffect, useMemo, useState } from "react";
import { branchOf } from "../../lib/agents";
import { ignore } from "../../lib/ignore";
import { buildTree, defaultOpen, flattenTree } from "../../lib/tree";
import { useStore } from "../../store";

/** Read-only checkout browser: `list_checkout_tree` folded into a tree, files
 *  open in the file viewer. No writes are exposed to the phone. */
export function CodeTab({ agent }: { agent: AgentRecord }) {
  const files = useStore((s) => s.trees[agent.id]);
  const loadTree = useStore((s) => s.loadTree);
  const push = useStore((s) => s.push);
  const [open, setOpen] = useState<Set<string> | null>(null);

  useEffect(() => {
    if (!files) void loadTree(agent.id).catch(ignore);
  }, [agent.id, files, loadTree]);

  const tree = useMemo(() => buildTree(files ?? []), [files]);
  useEffect(() => {
    if (tree.length && open === null) setOpen(defaultOpen(tree));
  }, [tree, open]);

  const rows = useMemo(() => flattenTree(tree, open ?? new Set()), [tree, open]);
  const toggle = (path: string) =>
    setOpen((prev) => {
      const next = new Set(prev ?? []);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  return (
    <div className="scroll">
      <div className="crumb">
        <Icon name="branch" size={11} />
        <b>{branchOf(agent)}</b>
        <span>·</span>
        <span>{agent.repos[0]?.repo_path ?? ""}</span>
      </div>
      {!files && <div className="empty">Loading the checkout…</div>}
      {files && files.length === 0 && (
        <div className="empty">
          <b>Nothing to show</b>
          The checkout is empty or unavailable.
        </div>
      )}
      <div className="tree">
        <div className="card">
          {rows.map((r) => (
            <button
              type="button"
              key={r.path}
              className={`tr${r.dir ? " dir" : ""}${r.open ? " open" : ""}`}
              style={
                {
                  paddingLeft: 12 + r.depth * 16,
                  "--x": `${12 + r.depth * 16}px`,
                } as React.CSSProperties
              }
              onClick={() =>
                r.dir ? toggle(r.path) : push("file", { agentId: agent.id, path: r.path })
              }
            >
              <span className={r.dir ? "fo" : "fi"}>
                <Icon name={r.dir ? "chevR" : "file"} size={13} />
              </span>
              <span className="nm">{r.name}</span>
              {!r.dir && r.status && (
                <span className={`m${r.status === "A" ? " u" : ""}`}>{r.status}</span>
              )}
              {r.dir && r.changed && !r.open && (
                <span className="dot" style={{ width: 6, height: 6, background: "var(--warn)" }} />
              )}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
