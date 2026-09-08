// Fixture host state for the mock. Session records are real Claude/Codex
// stream-json shapes (the same ones tests/adapters/**/fixtures carry), so the
// desktop adapters render them exactly as they render a live host's.

import type { AgentRecord, TrackedRepo, Workspace } from "@desktop/api/types/agent";
import type { CheckoutFile, CheckoutFileContents, DirEntry } from "@desktop/api/types/checkout";
import type { GitState } from "@desktop/api/types/git";
import type { PrChecks, PrState } from "@desktop/api/types/pr";
import type { GhRepoSummary, GhStatus } from "@desktop/api/types/providers";
import type { SessionRecord, UserTurn } from "@desktop/api/types/session";
import type { AgentModels } from "@desktop/data/modelCatalog/types";

export const FLETCH_REPO = "/Users/alex/.fletch/workspaces/fletch";
export const ATLAS_REPO = "/Users/alex/.fletch/workspaces/atlas";

/** The tool_use id the waiting agent is paused on, and the control-protocol
 *  request id the host is holding for it. */
export const PENDING_TOOL_USE_ID = "toolu_push_01";
export const PENDING_REQUEST_ID = "req_push_01";

const repo = (
  path: string,
  branch: string,
  parent: string,
  pr?: Partial<TrackedRepo>,
): TrackedRepo => ({
  repo_path: path,
  subdir: path.split("/").pop() ?? "repo",
  branch,
  parent_branch: parent,
  label: null,
  ...pr,
});

const agent = (a: Partial<AgentRecord> & Pick<AgentRecord, "id" | "name" | "project_id">) =>
  ({
    provider: "claude",
    repos: [repo(FLETCH_REPO, `feat/${a.name}`, "main")],
    task: "",
    status: "idle",
    view: "custom",
    session_id: `sess-${a.id}`,
    created_at: "2026-09-07T09:12:00Z",
    last_error: null,
    archive: null,
    effort: "high",
    model: "claude-fable-5-1",
    custom_agent_id: null,
    sandbox_engine: "sandbox-exec",
    issue_ref: null,
    purpose: null,
    ...a,
  }) as AgentRecord;

export const workspace: Workspace = {
  repos: [FLETCH_REPO, ATLAS_REPO],
  projects: [
    { path: FLETCH_REPO, name: "fletch", project_id: "prj-fletch", label: null },
    { path: ATLAS_REPO, name: "atlas", project_id: "prj-atlas", label: null },
  ],
  agents: [
    agent({
      id: "arabia",
      name: "arabia",
      project_id: "prj-fletch",
      status: "running",
      task: "Put dictation model choice in general settings",
      repos: [repo(FLETCH_REPO, "feat/dictation-models", "main")],
    }),
    agent({
      id: "pamukkale",
      name: "pamukkale",
      project_id: "prj-fletch",
      status: "running",
      task: "Fix the onboarding readiness flicker",
      repos: [repo(FLETCH_REPO, "fix/onboarding-flicker", "main")],
    }),
    agent({
      id: "kamakura",
      name: "kamakura",
      project_id: "prj-fletch",
      status: "idle",
      task: "Enable dictation for the composer",
      repos: [
        repo(FLETCH_REPO, "fix/dictation-followups", "main", {
          pr_number: 642,
          pr_url: "https://github.com/fwdai/fletch/pull/642",
          pr_title: "fix(dictation): key every event on a session id",
          pr_state: "open",
        }),
      ],
    }),
    agent({
      id: "caspian",
      name: "caspian",
      project_id: "prj-atlas",
      provider: "codex",
      model: "gpt-5.4-codex",
      effort: "medium",
      status: "error",
      last_error: "stream error: 429 quota exhausted",
      task: "Virtualize the Today list",
      repos: [repo(ATLAS_REPO, "perf/list-virtualization", "main")],
    }),
  ],
};

// --- session records ------------------------------------------------------

let seq = 0;
const rec = (provider: string, body: Record<string, unknown>): SessionRecord => {
  seq += 1;
  return {
    seq,
    provider,
    source: "transcript",
    native_id: `n${seq}`,
    agent_version: "2.0.31",
    body,
  };
};

const userText = (text: string) => ({
  type: "user",
  message: { role: "user", content: [{ type: "text", text }] },
});

const assistantText = (text: string) => ({
  type: "assistant",
  message: { role: "assistant", model: "claude-fable-5-1", content: [{ type: "text", text }] },
});

const assistantTool = (id: string, name: string, input: unknown) => ({
  type: "assistant",
  message: {
    role: "assistant",
    model: "claude-fable-5-1",
    content: [{ type: "tool_use", id, name, input }],
  },
});

const toolResult = (id: string, content: string, isError = false) => ({
  type: "user",
  message: {
    role: "user",
    content: [{ type: "tool_result", tool_use_id: id, content, is_error: isError }],
  },
});

const turnEnd = () => ({
  type: "result",
  subtype: "success",
  is_error: false,
  usage: { input_tokens: 8421, output_tokens: 1902 },
});

export const records: Record<string, SessionRecord[]> = {
  arabia: [
    rec("claude", userText("Put dictation model choice in general settings.")),
    rec(
      "claude",
      assistantText(
        "Reading the code around settings and dictation so the new choice lands next to the existing engine toggle.",
      ),
    ),
    rec(
      "claude",
      assistantTool("toolu_a1", "Grep", { pattern: "dictation|SFSpeech", path: "src" }),
    ),
    rec("claude", toolResult("toolu_a1", "7 matches across 4 files")),
    rec(
      "claude",
      assistantTool("toolu_a2", "Read", {
        file_path: "src/components/SettingsScreen/GeneralPane.tsx",
      }),
    ),
    rec("claude", toolResult("toolu_a2", "212 lines read")),
  ],
  pamukkale: [
    rec(
      "claude",
      userText("For some reason I see a lot of not-ready flashes on the onboarding screen."),
    ),
    rec(
      "claude",
      assistantText(
        "The readiness flag settles a frame after first paint. I moved the gate into the loader and committed it; publishing needs a force push because the branch was rebased.",
      ),
    ),
    rec("claude", assistantTool("toolu_p1", "Edit", { file_path: "src/onboarding/Gate.tsx" })),
    rec("claude", toolResult("toolu_p1", "Applied 3 edits")),
    rec(
      "claude",
      assistantTool(PENDING_TOOL_USE_ID, "Bash", {
        command: "git push --force-with-lease origin fix/onboarding-flicker",
        description: "Publish the rebased fix",
      }),
    ),
  ],
  kamakura: [
    rec("claude", userText("Enable dictation for the composer.")),
    rec("claude", assistantTool("toolu_k1", "Edit", { file_path: "src/composer/dictation.ts" })),
    rec("claude", toolResult("toolu_k1", "Applied 5 edits")),
    rec("claude", assistantTool("toolu_k2", "Bash", { command: "bunx vitest run src/composer" })),
    rec("claude", toolResult("toolu_k2", "Test Files 4 passed · Tests 27 passed · 2.8s")),
    rec(
      "claude",
      assistantText(
        "Fixed, pushed to PR #642, and the review thread is answered and resolved. Every dictation event is now keyed on a session id.",
      ),
    ),
    rec("claude", turnEnd()),
  ],
  // Codex records are rollout-file lines (session_meta / event_msg /
  // response_item), which is what its `normalizeTranscript` reads.
  caspian: [
    rec("codex", {
      type: "session_meta",
      payload: { id: "019e8cc9", cwd: ATLAS_REPO, cli_version: "0.135.0" },
    }),
    rec("codex", { type: "turn_context", payload: { cwd: ATLAS_REPO, model: "gpt-5.4-codex" } }),
    rec("codex", { type: "event_msg", payload: { type: "task_started", turn_id: "t1" } }),
    rec("codex", {
      type: "event_msg",
      payload: {
        type: "user_message",
        message: "Virtualize the Today list — 20k rows drops frames.",
        images: [],
      },
    }),
    rec("codex", {
      type: "response_item",
      payload: {
        type: "function_call",
        name: "exec_command",
        arguments: '{"cmd":"pnpm bench today-list --rows 20000","workdir":"."}',
        call_id: "call_1",
      },
    }),
    rec("codex", {
      type: "response_item",
      payload: {
        type: "function_call_output",
        call_id: "call_1",
        output: "Process exited with code 0\nOutput:\nscroll frame p95: 41ms\n",
      },
    }),
    rec("codex", {
      type: "event_msg",
      payload: {
        type: "agent_message",
        message: "Baseline p95 is 41 ms. Swapping in a windowed renderer next.",
      },
    }),
  ],
};

export const userTurns: Record<string, UserTurn[]> = {
  arabia: [
    {
      turn_id: "t-arabia-1",
      seq: 1,
      text: "Put dictation model choice in general settings.",
      attachments: [],
      native_id: "n1",
      started_at: Date.now() - 187_000,
      ended_at: null,
    },
  ],
  pamukkale: [],
  kamakura: [],
  caspian: [],
};

// --- git / PR -------------------------------------------------------------

const gitState = (
  branch: string,
  files: GitState["files"],
  extra?: Partial<GitState>,
): GitState => ({
  branch,
  parent_branch: "main",
  ahead: files.length ? 0 : 3,
  behind: 0,
  unpushed: files.length ? 0 : 1,
  files,
  additions: files.reduce((n, f) => n + f.additions, 0),
  deletions: files.reduce((n, f) => n + f.deletions, 0),
  remote_url: "https://github.com/fwdai/fletch",
  has_origin: true,
  head_sha: "5c2e8a1f",
  ...extra,
});

export const gitStates: Record<string, GitState> = {
  arabia: gitState("feat/dictation-models", [
    {
      path: "src/components/SettingsScreen/GeneralPane.tsx",
      kind: "modified",
      staged: false,
      additions: 48,
      deletions: 2,
    },
    {
      path: "src/components/Composer/dictation/useDictation.ts",
      kind: "modified",
      staged: false,
      additions: 21,
      deletions: 6,
    },
    {
      path: "src/components/SettingsScreen/EnginePicker.tsx",
      kind: "added",
      staged: false,
      additions: 67,
      deletions: 0,
    },
  ]),
  pamukkale: gitState("fix/onboarding-flicker", [
    {
      path: "src/onboarding/Gate.tsx",
      kind: "modified",
      staged: false,
      additions: 42,
      deletions: 17,
    },
  ]),
  kamakura: gitState("fix/dictation-followups", []),
  caspian: gitState("perf/list-virtualization", [
    { path: "src/today/List.tsx", kind: "modified", staged: false, additions: 88, deletions: 41 },
  ]),
};

export const prStates: Record<string, PrState> = {
  kamakura: {
    number: 642,
    url: "https://github.com/fwdai/fletch/pull/642",
    state: "open",
    title: "fix(dictation): key every event on a session id",
    mergeable: "mergeable",
  },
};

export const prChecks: Record<string, PrChecks> = {
  kamakura: {
    merge_state: "clean",
    rollup: "passing",
    total: 14,
    passed: 14,
    failed: 0,
    pending: 0,
    required_failing: [],
    runs: [
      {
        name: "build",
        status: "completed",
        conclusion: "success",
        required: true,
        url: null,
        started_at: null,
        completed_at: null,
      },
      {
        name: "test",
        status: "completed",
        conclusion: "success",
        required: true,
        url: null,
        started_at: null,
        completed_at: null,
      },
    ],
  },
};

// --- checkout tree / files ------------------------------------------------

export const checkoutTree: CheckoutFile[] = [
  { path: "package.json", status: null, additions: 0, deletions: 0 },
  { path: "src/main.tsx", status: null, additions: 0, deletions: 0 },
  {
    path: "src/components/SettingsScreen/GeneralPane.tsx",
    status: "M",
    additions: 48,
    deletions: 2,
  },
  {
    path: "src/components/SettingsScreen/EnginePicker.tsx",
    status: "A",
    additions: 67,
    deletions: 0,
  },
  {
    path: "src/components/Composer/dictation/useDictation.ts",
    status: "M",
    additions: 21,
    deletions: 6,
  },
  { path: "src/onboarding/Gate.tsx", status: null, additions: 0, deletions: 0 },
  { path: "src/today/List.tsx", status: null, additions: 0, deletions: 0 },
  { path: "docs/remote-protocol.md", status: null, additions: 0, deletions: 0 },
];

const SAMPLE_FILE = `import { useSetting } from "@/storage/settings";
import { useSystemRecognizer } from "./useSystemRecognizer";
import { useWhisper } from "./useWhisper";

export type Engine = "system" | "whisper";

export function useDictation() {
  const engine = useSetting<Engine>("dictation.engine");
  const system = useSystemRecognizer();
  const recognizer = system.available || engine !== "system" ? system : useWhisper("small");
  return recognizer;
}
`;

export const fileContents = (path: string): CheckoutFileContents => ({
  text: SAMPLE_FILE,
  lang: path.endsWith(".md") ? "markdown" : "typescript",
  status: checkoutTree.find((f) => f.path === path)?.status ?? null,
  chg_add: [8, 9, 10],
  chg_mod: [7],
  binary: false,
  too_large: false,
});

export const fileDiff = (path: string): string => `diff --git a/${path} b/${path}
index 3f1c9a2..8a91c04 100644
--- a/${path}
+++ b/${path}
@@ -6,7 +6,10 @@ export type Engine = "system" | "whisper";
 export function useDictation() {
   const engine = useSetting<Engine>("dictation.engine");
-  const recognizer = useSystemRecognizer();
+  const system = useSystemRecognizer();
+  const recognizer = system.available || engine !== "system"
+    ? system
+    : useWhisper("small");
   return recognizer;
 }
`;

export const branches: Record<string, string[]> = {
  [FLETCH_REPO]: [
    "main",
    "develop",
    "release/0.9",
    "feat/dictation",
    "fix/onboarding-flicker",
    "perf/list-virtualization",
  ],
  [ATLAS_REPO]: ["main", "develop", "perf/list-virtualization"],
};

export const supportedModels: AgentModels[] = [
  {
    agent: "claude",
    providerHint: "anthropic",
    models: [
      {
        id: "claude-fable-5-1",
        name: "Claude Fable 5.1",
        contextWindow: 1_000_000,
        reasoning: true,
      },
      { id: "claude-opus-5", name: "Claude Opus 5", contextWindow: 1_000_000, reasoning: true },
      { id: "claude-sonnet-5", name: "Claude Sonnet 5", contextWindow: 1_000_000, reasoning: true },
      {
        id: "claude-haiku-4-5",
        name: "Claude Haiku 4.5",
        contextWindow: 200_000,
        reasoning: false,
      },
    ],
  },
  {
    agent: "codex",
    models: [
      {
        id: "gpt-5.4-codex",
        name: "GPT-5.4 Codex",
        contextWindow: 400_000,
        reasoning: true,
        reasoningLevels: ["low", "medium", "high", "xhigh"],
        defaultReasoning: "medium",
      },
      { id: "gpt-5.4", name: "GPT-5.4", contextWindow: 400_000, reasoning: true },
    ],
  },
];

export const hostInfo = { name: "Alex's MacBook Pro", appVersion: "0.7.23", os: "macos" };

/** What `~` expands to on the fake host. */
export const HOME = "/Users/alex";

const dir = (name: string): DirEntry => ({ name, is_dir: true });
const file = (name: string): DirEntry => ({ name, is_dir: false });

/** A small fake filesystem for `list_dir`, so the folder picker has somewhere
 *  real-looking to walk in the browser dev loop. Only the interesting nodes are
 *  listed: a directory its parent names but that has no row of its own reads as
 *  empty (see `MockHost.listDir`), which keeps this table short. Files are in
 *  here too, because filtering them out is part of the picker's job. */
export const filesystem: Record<string, DirEntry[]> = {
  "/": [dir("Applications"), dir("Users"), dir("tmp")],
  "/Users": [dir("alex"), dir("Shared")],
  [HOME]: [
    dir(".config"),
    dir(".fletch"),
    dir(".ssh"),
    dir("Code"),
    dir("Documents"),
    dir("Downloads"),
    file(".zshrc"),
  ],
  [`${HOME}/.config`]: [dir("gh")],
  [`${HOME}/.config/gh`]: [file("hosts.yml")],
  [`${HOME}/.fletch`]: [dir("workspaces")],
  [`${HOME}/.fletch/workspaces`]: [dir("atlas"), dir("fletch"), dir("uluru")],
  [FLETCH_REPO]: [dir("docs"), dir("mobile"), dir("src"), file("package.json")],
  [ATLAS_REPO]: [dir("src"), file("Cargo.toml")],
  [`${HOME}/Code`]: [dir("playground"), dir("scratch"), file("README.md")],
  [`${HOME}/Code/playground`]: [dir("src"), file("README.md")],
  [`${HOME}/Downloads`]: [file("fletch-0.7.23.dmg")],
};

export const ghStatus: GhStatus = { installed: true, authenticated: true, login: "alexchaplinsky" };

export const ghRepos: GhRepoSummary[] = [
  {
    name_with_owner: "fwdai/fletch",
    description: "Agent workspaces for the Mac",
    is_private: true,
    updated_at: "2026-09-08T08:41:00Z",
  },
  {
    name_with_owner: "fwdai/atlas",
    description: "Rust core for the model catalog",
    is_private: true,
    updated_at: "2026-09-06T17:02:00Z",
  },
  {
    name_with_owner: "fwdai/relay",
    description: "Cloudflare Worker relay for off-network access",
    is_private: false,
    updated_at: "2026-09-05T11:20:00Z",
  },
  {
    name_with_owner: "alexchaplinsky/dotfiles",
    description: null,
    is_private: false,
    updated_at: "2026-08-22T09:15:00Z",
  },
  {
    name_with_owner: "alexchaplinsky/geist-playground",
    description: "Type experiments",
    is_private: false,
    updated_at: "2026-07-30T14:48:00Z",
  },
];
