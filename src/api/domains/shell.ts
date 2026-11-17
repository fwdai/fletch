import type { Invoke } from "../invoke";

export const shellApi = (invoke: Invoke) => ({
  openAgentShell: (agentId: string) => invoke<void>("open_agent_shell", { agentId }),
  closeAgentShell: (agentId: string) => invoke<void>("close_agent_shell", { agentId }),
  writeToShell: (agentId: string, data: string) =>
    invoke<void>("write_to_shell", { agentId, data }),
  resizeShell: (agentId: string, cols: number, rows: number) =>
    invoke<void>("resize_shell", { agentId, cols, rows }),
});
