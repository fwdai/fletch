// Scripted turns for the mock host. Each step carries the live wire event the
// host would forward on `agent:event` and, where the agent would persist one,
// the canonical session record that lands afterwards — both in the provider's
// real shape, so the desktop adapters do the rendering.

export interface ScriptStep {
  live: Record<string, unknown>;
  record?: Record<string, unknown>;
}

const streamText = (text: string): ScriptStep => ({
  live: {
    type: "stream_event",
    event: { type: "content_block_start", content_block: { type: "text", text } },
  },
});

const claudeText = (text: string): ScriptStep => ({
  live: {
    type: "assistant",
    message: { role: "assistant", model: "claude-fable-5-1", content: [{ type: "text", text }] },
  },
  record: {
    type: "assistant",
    message: { role: "assistant", model: "claude-fable-5-1", content: [{ type: "text", text }] },
  },
});

const claudeTool = (id: string, name: string, input: unknown, output: string): ScriptStep[] => [
  {
    live: {
      type: "assistant",
      message: {
        role: "assistant",
        model: "claude-fable-5-1",
        content: [{ type: "tool_use", id, name, input }],
      },
    },
    record: {
      type: "assistant",
      message: {
        role: "assistant",
        model: "claude-fable-5-1",
        content: [{ type: "tool_use", id, name, input }],
      },
    },
  },
  {
    live: {
      type: "user",
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: id, content: output }],
      },
    },
    record: {
      type: "user",
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: id, content: output }],
      },
    },
  },
];

const claudeEnd = (): ScriptStep => ({
  live: {
    type: "result",
    subtype: "success",
    is_error: false,
    usage: { input_tokens: 12_004, output_tokens: 890 },
  },
  record: {
    type: "result",
    subtype: "success",
    is_error: false,
    usage: { input_tokens: 12_004, output_tokens: 890 },
  },
});

const codexStep = (
  payload: Record<string, unknown>,
  live: Record<string, unknown>,
): ScriptStep => ({
  live,
  record: { type: "response_item", payload },
});

function claudeScript(prompt: string): ScriptStep[] {
  const suffix = prompt.length > 60 ? `${prompt.slice(0, 57)}…` : prompt;
  const id = `toolu_${Math.random().toString(36).slice(2, 8)}`;
  const opener = `On it: ${suffix}`;
  return [
    // The streamed text is the same string the finalized event carries, as
    // Claude really sends it — the reducer then stamps the model onto the
    // existing message instead of appending a second paragraph.
    streamText(opener),
    claudeText(opener),
    ...claudeTool(
      `${id}_a`,
      "Grep",
      { pattern: "dictation|engine", path: "src" },
      "7 matches across 4 files",
    ),
    ...claudeTool(
      `${id}_b`,
      "Edit",
      { file_path: "src/components/SettingsScreen/GeneralPane.tsx" },
      "Applied 4 edits (+48 −2)",
    ),
    ...claudeTool(
      `${id}_c`,
      "Bash",
      { command: "bunx vitest run src/components", description: "Run the pane tests" },
      "Test Files 4 passed · Tests 27 passed · 2.8s",
    ),
    claudeText(
      "Done. The setting defaults to the system recognizer and falls back to Whisper small where it is unavailable. Tests are green.",
    ),
    claudeEnd(),
  ];
}

function codexScript(prompt: string): ScriptStep[] {
  return [
    codexStep(
      {
        type: "message",
        role: "assistant",
        content: [{ type: "output_text", text: `Resuming: ${prompt}` }],
      },
      {
        type: "item.completed",
        item: { id: "i1", type: "agent_message", text: `Resuming: ${prompt}` },
      },
    ),
    codexStep(
      {
        type: "function_call",
        name: "exec_command",
        arguments: '{"cmd":"pnpm bench today-list --rows 20000","workdir":"."}',
        call_id: "call_r1",
      },
      {
        type: "item.completed",
        item: {
          id: "i2",
          type: "command_execution",
          command: "pnpm bench today-list --rows 20000",
          aggregated_output: "scroll frame p95: 9.8ms (was 41ms)\n",
          exit_code: 0,
          status: "completed",
        },
      },
    ),
    {
      live: {
        type: "item.completed",
        item: {
          id: "i3",
          type: "agent_message",
          text: "Virtualization holds at 20k rows — p95 frame under 10 ms.",
        },
      },
      record: {
        type: "event_msg",
        payload: {
          type: "agent_message",
          message: "Virtualization holds at 20k rows — p95 frame under 10 ms.",
        },
      },
    },
    {
      live: { type: "turn.completed", usage: { input_tokens: 61_043, output_tokens: 179 } },
      record: { type: "event_msg", payload: { type: "task_complete", turn_id: "t1" } },
    },
  ];
}

/** A top-level `system` frame — the task bookkeeping Claude emits for
 *  background sub-agents. Never persisted, so no record. */
const system = (subtype: string, fields: Record<string, unknown>): ScriptStep => ({
  live: { type: "system", subtype, ...fields },
});

/** A sidechain frame: the same wire shape, tagged with the Agent tool_use that
 *  spawned the sub-agent — which is how the reducer threads it under that row. */
const asChild = (step: ScriptStep, parentToolUseId: string): ScriptStep => ({
  live: { ...step.live, parent_tool_use_id: parentToolUseId },
});

/** Type a prompt matching this into any Claude chat to preview sub-agents. */
const SUBAGENT_PROMPT = /sub-?agents?|delegate|in parallel/i;

/** A turn that backgrounds a sub-agent and ends before it does. Everything
 *  after `claudeEnd` is the tail a real host keeps forwarding while the main
 *  agent sits idle: the sub-agent's own tagged turns, its progress frames, and
 *  finally its notification. */
function claudeSubagentScript(prompt: string): ScriptStep[] {
  const suffix = prompt.length > 60 ? `${prompt.slice(0, 57)}…` : prompt;
  const id = `toolu_${Math.random().toString(36).slice(2, 8)}`;
  const agentToolUse = `${id}_agent`;
  const taskId = `task_${id.slice(6)}`;
  const description = "Audit the store for stale busy flags";
  const opener = `Handing part of this to a sub-agent: ${suffix}`;
  const [launch, launched] = claudeTool(
    agentToolUse,
    "Agent",
    {
      description,
      prompt: "Find every place the busy flag can go stale.",
      subagent_type: "Explore",
    },
    `Async agent launched successfully.\nagentId: ${taskId}\nStatus: running`,
  );
  const [grep, grepped] = claudeTool(
    `${id}_s1`,
    "Grep",
    { pattern: "busy", path: "mobile/src/store" },
    "9 matches across 3 files",
  );
  const [read, readOut] = claudeTool(
    `${id}_s2`,
    "Read",
    { file_path: "mobile/src/store/index.ts" },
    "function reconcileBusy(busy, ws) { … }",
  );
  const liveList = [{ task_id: taskId, task_type: "local_agent", description }];
  return [
    streamText(opener),
    claudeText(opener),
    launch,
    system("task_started", {
      task_id: taskId,
      tool_use_id: agentToolUse,
      description,
      task_type: "local_agent",
      subagent_type: "Explore",
      is_backgrounded: true,
      spawn_depth: 0,
    }),
    launched,
    system("background_tasks_changed", { tasks: liveList }),
    claudeText("The audit is running in the background; I'll pick up its report when it lands."),
    claudeEnd(),
    asChild(claudeText("Reading the store first."), agentToolUse),
    asChild(grep, agentToolUse),
    system("task_progress", {
      task_id: taskId,
      description: "Grep mobile/src/store",
      usage: { tool_uses: 1, total_tokens: 1_840, duration_ms: 2_100 },
      last_tool_name: "Grep",
    }),
    asChild(grepped, agentToolUse),
    asChild(read, agentToolUse),
    system("task_progress", {
      task_id: taskId,
      description: "Read mobile/src/store/index.ts",
      usage: { tool_uses: 2, total_tokens: 4_310, duration_ms: 6_800 },
      last_tool_name: "Read",
    }),
    asChild(readOut, agentToolUse),
    asChild(
      claudeText(
        "Report: `reconcileBusy` already clears flags a snapshot contradicts; the only gap is a turn:sent mirror that arrives after a reconnect.",
      ),
      agentToolUse,
    ),
    system("task_notification", {
      task_id: taskId,
      status: "completed",
      summary: "One gap: the turn:sent mirror after a reconnect.",
      output_file: `/tmp/claude/tasks/${taskId}.md`,
      usage: { tool_uses: 2, total_tokens: 6_320, duration_ms: 11_400 },
    }),
    system("background_tasks_changed", { tasks: [] }),
  ];
}

export function scriptFor(provider: string, prompt: string): ScriptStep[] {
  if (provider === "codex") return codexScript(prompt);
  return SUBAGENT_PROMPT.test(prompt) ? claudeSubagentScript(prompt) : claudeScript(prompt);
}
