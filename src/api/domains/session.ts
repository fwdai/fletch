import { invoke } from "../invoke";
import type { SessionRecord, SupersededSession, UserTurn } from "../types/session";

export const sessionApi = {
  readSessionRecords: (agentId: string) =>
    invoke<SessionRecord[]>("read_session_records", { agentId }),
  /** Every session the agent's workspace has superseded — a rewind's
   *  abandoned branches — oldest first, for its spend. Those in `known` come
   *  without their records: a superseded session never changes. */
  readSupersededRecords: (agentId: string, known: string[]) =>
    invoke<SupersededSession[]>("read_superseded_records", { agentId, known }),
  readUserTurns: (agentId: string) => invoke<UserTurn[]>("read_user_turns", { agentId }),
  syncSession: (agentId: string) => invoke<void>("sync_session", { agentId }),
  /** Persist a runtime-compiled record (e.g. cursor's per-turn usage from its
   *  live `result` event) into session_records. Idempotent on `nativeId`. */
  appendLiveRecord: (
    agentId: string,
    provider: string,
    nativeId: string,
    body: Record<string, unknown>,
  ) => invoke<boolean>("append_live_record", { agentId, provider, nativeId, body }),
};
