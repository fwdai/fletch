import type { ChatAdapter } from "../types";
import { reduce } from "./reduce";
import { normalizeTranscript } from "./normalize";
import { codexPolicy } from "./policy";

// TODO(codex-real-impl): see ./reduce.ts and ./normalize.ts. The shape
// of this adapter is real; the body is best-effort against public
// Responses-API docs and needs verification once codex's Rust transport
// is wired and we can observe its actual stdout shape.
export const codexAdapter: ChatAdapter = {
  id: "codex",
  reduce,
  normalizeTranscript,
  policy: codexPolicy,
};
