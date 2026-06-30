// Anthropic content-block helpers. Specific to claude's message shape;
// would not generalize to providers that don't use the same content-block
// envelope.

import { asBlockList } from "@/adapters/shared/json";

/** Extract joined text from a string or content-block array. */
export function contentText(content: unknown): string {
  if (typeof content === "string") return content;
  return asBlockList(content)
    .map((block) => {
      if (block.type === "text" && typeof block.text === "string") {
        return block.text;
      }
      return "";
    })
    .filter(Boolean)
    .join("\n");
}

/** Like contentText but trims each block; used for transcript replay where
 *  whitespace at edges varies more than in live streams. */
export function transcriptTextContent(content: unknown): string {
  if (typeof content === "string") return content.trim();
  return asBlockList(content)
    .filter((block) => block.type === "text" && typeof block.text === "string")
    .map((block) => String(block.text).trim())
    .filter(Boolean)
    .join("\n");
}
