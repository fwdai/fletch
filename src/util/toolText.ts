// Plain-text views of a tool call's payloads, shared by the chat's tool
// presenters and the handoff transcript (adapters/handoff).

/** Flatten a tool_result.content payload to text. Accepts strings,
 *  anthropic content-block arrays, or arbitrary JSON. */
export function renderToolResult(content: unknown): string {
  if (content == null) return "";
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((block) => {
        if (block && typeof block === "object" && "text" in block) {
          return String((block as { text: unknown }).text ?? "");
        }
        return typeof block === "string" ? block : JSON.stringify(block);
      })
      .join("\n");
  }
  return JSON.stringify(content, null, 2);
}

/** Best-effort string view of tool call input. Used by the default presenter
 *  and as a fallback inside specialized ones. */
export function stringifyInput(input: unknown, indent = 0): string {
  if (input == null) return "";
  if (typeof input === "string") return input;
  try {
    return JSON.stringify(input, null, indent);
  } catch {
    return "";
  }
}
