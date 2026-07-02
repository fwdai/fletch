import type { ChatAdapter } from "@/adapters/types";
import { normalizeTranscript } from "./normalize";
import { antigravityPolicy } from "./policy";
import { reduce } from "./reduce";

// Antigravity (agy). Unlike the other agents it has no live JSON event stream —
// its turn runner is plaintext — so history comes entirely from its on-disk
// transcript (see ./normalize.ts), ingested into session_records at turn-end and
// replayed through normalizeTranscript → reduce.
export const antigravityAdapter: ChatAdapter = {
  id: "antigravity",
  reduce,
  normalizeTranscript,
  policy: antigravityPolicy,
};
