import { useEffect, useState } from "react";
import { api } from "@/api";
import type { CodeState } from "./options";

/** What restoring the code to before `turnId` would do, asked of the backend
 *  when the menu opens: the preview the confirmation lists, or why the code
 *  can't be restored. Not asked while `ask` is false (no rewind can start). */
export function useCodeState(agentId: string, turnId: string, ask: boolean): CodeState {
  const [code, setCode] = useState<CodeState>("checking");
  useEffect(() => {
    if (!ask) return;
    let current = true;
    api.previewRewindCode(agentId, turnId).then(
      (report) => current && setCode({ report }),
      (e) => current && setCode({ unavailable: String(e) }),
    );
    return () => {
      current = false;
    };
  }, [agentId, turnId, ask]);
  return code;
}
