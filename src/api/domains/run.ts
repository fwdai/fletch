import { invoke } from "../invoke";
import type { DetectedConfig, EnvEntry, ProjectRunConfig, RunStateSnapshot } from "../types/run";
import type { VerificationReport } from "../types/verify";

export const runApi = {
  runStart: (agentId: string) => invoke<void>("run_start", { agentId }),
  runStop: (agentId: string) => invoke<void>("run_stop", { agentId }),
  runState: (agentId: string) => invoke<RunStateSnapshot>("run_state", { agentId }),
  detectRunConfig: (agentId: string) => invoke<DetectedConfig[]>("detect_run_config", { agentId }),
  /** Run the project's deterministic checks (install → test → lint) in an
   *  agent's checkout. `subdir` targets a specific tracked repo (primary when
   *  omitted). */
  runVerification: (agentId: string, subdir?: string) =>
    invoke<VerificationReport>("run_verification", { agentId, subdir: subdir ?? null }),
  projectRunConfig: (repoPath: string) =>
    invoke<ProjectRunConfig>("project_run_config", { repoPath }),
  readEnvFileKeys: (repoPath: string) => invoke<EnvEntry[]>("read_env_file_keys", { repoPath }),
  getEnvOverride: (projectId: string, key: string) =>
    invoke<string | null>("get_env_override", { projectId, key }),
  setEnvOverride: (projectId: string, key: string, value: string) =>
    invoke<void>("set_env_override", { projectId, key, value }),
  clearEnvOverride: (projectId: string, key: string) =>
    invoke<void>("clear_env_override", { projectId, key }),
};
