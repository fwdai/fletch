// Claude's web tools. A fetch reads one page; a search looks something up.
// Their rows name the host or the query instead of a key/value dump.

import { renderToolResult } from "@/util/toolText";
import { Caption, CodeSurface } from "./blocks";
import type { ToolPresenter } from "./types";
import { firstLineOf, getStringField } from "./util";

function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

export const webFetchPresenter: ToolPresenter = {
  icon: "fetch",
  title: "Fetch",
  summary: (call) => {
    const url = getStringField(call.input, "url");
    return url ? firstLineOf(url.replace(/^https?:\/\//, ""), 120) : "(no url)";
  },
  expanded: (call, result) => {
    const url = getStringField(call.input, "url");
    const prompt = getStringField(call.input, "prompt");
    return (
      <>
        {url && (
          <Caption>
            <span className="tb-mono">{url}</span>
          </Caption>
        )}
        {prompt && <blockquote className="tb-quote">{prompt}</blockquote>}
        {result && (
          <CodeSurface text={renderToolResult(result.content)} isError={result.is_error} />
        )}
      </>
    );
  },
  facts: (call) => ({ kind: "read", path: hostOf(getStringField(call.input, "url")) }),
  activity: (call) => {
    const url = getStringField(call.input, "url");
    return url ? `Reading ${hostOf(url)}` : "Reading a page";
  },
};

export const webSearchPresenter: ToolPresenter = {
  icon: "fetch",
  title: "Web search",
  summary: (call) => getStringField(call.input, "query") || "(no query)",
  expanded: (call, result) => (
    <>
      <blockquote className="tb-quote">{getStringField(call.input, "query")}</blockquote>
      {result && <CodeSurface text={renderToolResult(result.content)} isError={result.is_error} />}
    </>
  ),
  facts: (call) => ({ kind: "search", subject: getStringField(call.input, "query") }),
  activity: (call) => {
    const query = getStringField(call.input, "query");
    return query ? `Searching the web for ${firstLineOf(query, 48)}` : "Searching the web";
  },
};
