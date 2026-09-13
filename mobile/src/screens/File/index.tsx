import type { CheckoutFileContents } from "@desktop/api/types/checkout";
import { Icon } from "@desktop/components/Icon";
import { useEffect, useState } from "react";
import { CodeLine, Nav } from "../../components/ui";
import { api, useStore } from "../../store";

/** Read-only file viewer. `chg_add` / `chg_mod` are the 1-indexed lines the
 *  agent added / modified, which is what tints the gutter. */
export function FileScreen({ agentId, path }: { agentId: string; path: string }) {
  const pop = useStore((s) => s.pop);
  const push = useStore((s) => s.push);
  const changed = useStore((s) => s.gitStates[agentId]?.files.some((f) => f.path === path));
  const [file, setFile] = useState<CheckoutFileContents | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .readCheckoutFile(agentId, path)
      .then((f) => live && setFile(f))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, [agentId, path]);

  const parts = path.split("/");
  const name = parts.pop();
  const added = new Set(file?.chg_add ?? []);
  const modified = new Set(file?.chg_mod ?? []);

  return (
    <>
      <Nav
        onBack={pop}
        backLabel="Code"
        title={name ?? path}
        sub={parts.join("/") || "/"}
        hair
        right={
          changed ? (
            <button type="button" className="tbtn" onClick={() => push("diff", { agentId, path })}>
              Diff
            </button>
          ) : undefined
        }
      />
      <div className="scroll">
        {error && <div className="empty">{error}</div>}
        {!file && !error && <div className="empty">Reading…</div>}
        {file?.binary && <div className="empty">Binary file</div>}
        {file?.too_large && <div className="empty">File is too large to display</div>}
        {file && !file.binary && !file.too_large && (
          <div className="code">
            {file.text.split("\n").map((line, i) => {
              const n = i + 1;
              const cls = added.has(n) ? " hl" : modified.has(n) ? " mod" : "";
              return (
                // biome-ignore lint/suspicious/noArrayIndexKey: line number is the identity
                <div key={i} className={`ln${cls}`}>
                  <span className="n">{n}</span>
                  <span className="t">
                    <CodeLine text={line} />
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </div>
      {file && !file.binary && (
        <div className="crumb" style={{ paddingBottom: "calc(var(--safe-bottom) + 6px)" }}>
          <Icon name="file" size={11} />
          {file.lang}
          <span>·</span>
          {file.text.split("\n").length} lines
        </div>
      )}
    </>
  );
}
