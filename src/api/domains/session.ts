import { invoke } from "../invoke";
import type { SessionRecord, UserTurn } from "../types/session";

export const sessionApi = {
  readSessionRecords: (agentId: string) =>
    invoke<SessionRecord[]>("read_session_records", { agentId }),
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
