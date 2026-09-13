import { Icon } from "@desktop/components/Icon";
import { useEffect, useMemo, useState } from "react";
import { CodeLine, Nav } from "../../components/ui";
import { parseUnifiedDiff, STATUS_LETTER } from "../../lib/diff";
import { api, useStore } from "../../store";

/** One file's diff, with prev/next across the agent's changed files. */
export function DiffScreen({ agentId, path }: { agentId: string; path: string }) {
  const pop = useStore((s) => s.pop);
  const push = useStore((s) => s.push);
  // Selected as the whole git state, so the selector keeps a stable reference.
  const git = useStore((s) => s.gitStates[agentId]);
  const files = git?.files ?? [];
  const [current, setCurrent] = useState(path);
  const [diff, setDiff] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setDiff(null);
    api
      .getFileDiff(agentId, current)
      .then((d) => live && setDiff(d))
      .catch((e) => live && setDiff(`Could not read the diff: ${e}`));
    return () => {
      live = false;
    };
  }, [agentId, current]);

  const hunks = useMemo(() => (diff ? parseUnifiedDiff(diff) : []), [diff]);
  const index = files.findIndex((f) => f.path === current);
  const file = files[index];
  const parts = current.split("/");
  const name = parts.pop();

  return (
    <>
      <Nav
        onBack={pop}
        backLabel="Changes"
        title={
          <>
            {file && (
              <span className="mono" style={{ fontSize: 11, color: "var(--warn)" }}>
                {STATUS_LETTER[file.kind] ?? "M"}
              </span>
            )}
            {name ?? current}
          </>
        }
        sub={
          <>
            {file && (
              <>
                <span className="add">+{file.additions}</span>
                <span className="rem">−{file.deletions}</span>
              </>
            )}
            <span>{parts.join("/")}</span>
          </>
        }
        hair
        right={
          <button
            type="button"
            className="ibtn"
            onClick={() => push("file", { agentId, path: current })}
            aria-label="Open file"
          >
            <Icon name="code" size={18} />
          </button>
        }
      />
      <div className="scroll">
        {diff === null && <div className="empty">Reading the diff…</div>}
        {diff !== null && hunks.length === 0 && (
          <div className="empty">
            <b>No textual diff</b>
            The file may be binary, or the change is already committed.
          </div>
        )}
        <div className="diffview pane" key={current}>
          {hunks.map((h) => (
            <div key={h.header}>
              <div className="hunk">{h.header}</div>
              {h.lines.map((l, li) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: position within the hunk
                <div key={li} className={`dl ${l.kind}`}>
                  <span className="g">{l.old ?? ""}</span>
                  <span className="g">{l.next ?? ""}</span>
                  <span className="s">{l.kind === "add" ? "+" : l.kind === "rem" ? "−" : ""}</span>
                  <span className="t">
                    <CodeLine text={l.text} />
                  </span>
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>
      {files.length > 1 && index >= 0 && (
        <div className="diff-nav">
          <button
            type="button"
            disabled={index === 0}
            onClick={() => setCurrent(files[index - 1].path)}
          >
            <Icon name="chevL" size={16} />
            <span>{index > 0 ? files[index - 1].path.split("/").pop() : ""}</span>
          </button>
          <span className="c">
            {index + 1} / {files.length}
          </span>
          <button
            type="button"
            disabled={index === files.length - 1}
            onClick={() => setCurrent(files[index + 1].path)}
          >
            <span>{index < files.length - 1 ? files[index + 1].path.split("/").pop() : ""}</span>
            <Icon name="chevR" size={16} />
          </button>
        </div>
      )}
    </>
  );
}
