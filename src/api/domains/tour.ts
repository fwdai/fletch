import { invoke } from "../invoke";
import type { Tour } from "../types/tour";

export const tourApi = {
  /** The agent-authored walkthrough of the workspace's changes, or null when
   *  the agent has not submitted one. */
  getTour: (agentId: string) => invoke<Tour | null>("get_tour", { agentId }),
};
