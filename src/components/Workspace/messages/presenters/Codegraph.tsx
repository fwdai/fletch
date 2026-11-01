import { renderToolResult, stringifyInput } from "@/util/toolText";
import { Caption, CodeSurface, Fields, PathText } from "./blocks";
import type { ToolPresenter } from "./types";
import { firstLineOf, getStringField, SummaryNote } from "./util";

/** Pull the operation out of a codegraph tool name, regardless of how the
 *  adapter spelled the namespace: the tool itself is always `codegraph_<op>`,
 *  so the op is whatever follows the last `codegraph_`. Covers Claude's
 *  `mcp__codegraph__codegraph_explore`, Codex's `codegraph.codegraph_explore`,
 *  and `_`-joined variants (all -> "explore"). Falls back to the raw name for
 *  shapes we don't recognize. */
function codegraphOp(name: string): string {
  const marker = "codegraph_";
  const i = name.toLowerCase().lastIndexOf(marker);
  return i >= 0 ? name.slice(i + marker.length) : name;
}

/** Presenter for the codegraph MCP tools (`mcp__codegraph__*`) — a code
 *  knowledge graph queried by symbol/file names or a natural-language question.
 *  The raw tool name is noisy, so we badge these as "Codegraph" with a graph
 *  icon and surface the query (plus which operation ran). */
export const codegraphPresenter: ToolPresenter = {
  icon: "graph",
  title: "Codegraph",
  summary: (call) => {
    const query = getStringField(call.input, "query");
    const op = codegraphOp(call.name);
    return (
      <>
        {query || firstLineOf(stringifyInput(call.input), 120)}
        {op && <SummaryNote>{op}</SummaryNote>}
      </>
    );
  },
  expanded: (call, result) => {
    const query = getStringField(call.input, "query");
    const projectPath = getStringField(call.input, "projectPath");
    return (
      <>
        {query ? (
          <blockquote className="tb-quote">{query}</blockquote>
        ) : (
          <Fields input={call.input} />
        )}
        {projectPath && (
          <Caption>
            <PathText path={projectPath} />
          </Caption>
        )}
        {result && (
          <CodeSurface text={renderToolResult(result.content)} isError={result.is_error} />
        )}
      </>
    );
  },
  facts: (call) => ({ kind: "search", subject: getStringField(call.input, "query") }),
  activity: () => "Exploring the code graph",
};
