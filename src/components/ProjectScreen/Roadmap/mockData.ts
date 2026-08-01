// Placeholder roadmap content. There is no roadmap table or API yet, so the
// board renders from this module for every project. It is deliberately diverse
// — every horizon, size, source and status is represented, plus an epic, a
// dependency chain and an item an agent is actively working — so the shapes in
// `types.ts` can be turned into a schema without discovering missing columns.
//
// Replacing this with real data means swapping the four exports below for
// loaders; nothing else in the folder reads from it.

import type { MapDomain, PmBody, RoadmapItem, ScriptBeat } from "./types";

/** Codes minted for free-form ideas the PM captures mid-conversation. */
export const CODE_PREFIX = "FLT-";
export const FIRST_FREE_CODE = 160;

/** Items already merged. Not on the board — only the header stat. */
export const SHIPPED_COUNT = 18;

export const SEED_ITEMS: RoadmapItem[] = [
  {
    code: "FLT-142",
    horizon: "now",
    size: "M",
    area: "runtime",
    source: "linear",
    status: "active",
    agent: "dolomites",
    title: "Persist worktree state across app restarts",
    why: "Fletch forgets in-flight agents when the app quits — the loudest complaint of the last two weeks.",
    accept: [
      "Worktree registry survives a hard quit",
      "Reattaching shows the agent's last transcript",
      "Orphaned worktrees are offered for cleanup",
    ],
  },
  {
    code: "FLT-149",
    horizon: "now",
    size: "S",
    area: "runtime",
    source: "pm",
    status: "open",
    title: "Recover orphaned worktrees on launch",
    why: "Falls out of FLT-142 — without recovery the registry drifts from what's on disk.",
    accept: ["Scan .fletch/ on boot", "One-click adopt or delete"],
    deps: ["FLT-142"],
  },
  {
    code: "FLT-137",
    horizon: "next",
    size: "S",
    area: "runtime",
    source: "linear",
    status: "open",
    title: "Backpressure on agent output streaming",
    why: "Large diffs flood the renderer; the paint-jitter fix only masked it.",
    accept: ["Stream chunks capped per frame", "No dropped tokens under load"],
  },
  {
    code: "FLT-144",
    horizon: "next",
    size: "M",
    area: "chrome",
    source: "pm",
    status: "open",
    title: "Per-project agent presets",
    why: "Every new agent in a repo gets the same provider, base branch and workflow — stop re-picking it.",
    accept: ["Preset editable from the project page", "New-agent draft pre-filled"],
  },
  {
    code: "FLT-131",
    horizon: "next",
    size: "XS",
    area: "chrome",
    source: "github",
    status: "open",
    title: "Surface per-agent token spend in the title bar",
    why: "Spend is invisible until the invoice arrives.",
    accept: ["Live cost per worktree", "Roll-up per project"],
  },
  {
    code: "FLT-119",
    horizon: "later",
    size: "L",
    area: "platform",
    source: "pm",
    status: "open",
    title: "Multi-repo projects",
    why: "Teams with split front/back repos run two Fletch projects today and lose the shared roadmap.",
    accept: ["One project, many repos", "Agents pick a repo at spawn"],
  },
  {
    code: "FLT-124",
    horizon: "later",
    size: "M",
    area: "chrome",
    source: "pm",
    status: "open",
    title: "Ambient run monitoring outside the app",
    why: "People leave Fletch while agents work and want a glanceable signal.",
    accept: ["Menu-bar summary", "Notification when an agent needs you"],
  },
];

export const PRODUCT_MAP: MapDomain[] = [
  {
    id: "runtime",
    label: "Agent runtime",
    note: "spawner · worktrees · providers",
    files: 42,
    items: 3,
    heat: "hot",
  },
  {
    id: "chrome",
    label: "App chrome",
    note: "title bar · panels · settings",
    files: 61,
    items: 3,
    heat: "warm",
  },
  {
    id: "planning",
    label: "Planning",
    note: "backlog · roadmap · this page",
    files: 18,
    items: 1,
    heat: "warm",
  },
  {
    id: "vcs",
    label: "Git & review",
    note: "diffs · PRs · checks",
    files: 34,
    items: 0,
    heat: "cool",
  },
  {
    id: "platform",
    label: "Workflows",
    note: "steps · loops · runs",
    files: 27,
    items: 0,
    heat: "cool",
  },
];

/** What's already in the thread when the page opens. */
export const SEED_THREAD: PmBody[] = [
  {
    kind: "user",
    body: "The thing people keep hitting is that quitting Fletch loses everything in flight. Make that not happen.",
  },
  {
    kind: "probe",
    summary: "Read 9 files across the runtime · found 1 partial capability",
    findings: [
      {
        kind: "ok",
        text: "`WorktreeRegistry` already tracks live worktrees in memory — no persistence layer.",
      },
      {
        kind: "warn",
        text: "The reattach path assumes a live child process; it needs a resume branch.",
      },
      {
        kind: "ok",
        text: "`.fletch/` on disk is already the source of truth for the checkout itself.",
      },
    ],
  },
  {
    kind: "text",
    body: "That's a real gap, not a bug — the registry is memory-only. I filed it at the top of In flight and split the recovery half out, because the two land at different times.",
  },
  { kind: "landed", codes: ["FLT-142", "FLT-149"] },
];

/** Canned exchanges, offered as suggestion chips until they've been used. */
export const SCRIPT: ScriptBeat[] = [
  {
    prompt: "I'd like the app to export a project's roadmap as a shareable PDF.",
    msgs: [
      { kind: "thinking", body: "Checking whether anything already serializes the roadmap." },
      {
        kind: "probe",
        summary: "Read 4 files in planning · 1 reusable capability · 0 conflicts",
        findings: [
          {
            kind: "ok",
            text: "`serializeRoadmap()` already exists for the share link — shipped in FLT-118.",
          },
          { kind: "ok", text: "The changelog page's print stylesheet applies here unchanged." },
        ],
      },
      {
        kind: "text",
        body: "Straightforward. This is a renderer on top of a serializer you already ship, so it's one small item — no epic needed.",
      },
      {
        kind: "proposal",
        note: "1 addition",
        changes: [
          {
            kind: "add",
            item: {
              code: "FLT-151",
              horizon: "next",
              size: "S",
              area: "planning",
              source: "pm",
              status: "open",
              title: "Export roadmap as a shareable PDF",
              why: "Rides on the existing serializer and print stylesheet, so it's cheap and unblocks weekly stakeholder updates.",
              accept: [
                "Print view groups by horizon",
                "Includes owner and size",
                "One click from the project page",
              ],
            },
          },
        ],
      },
    ],
  },
  {
    prompt: "I want teams to be able to review agent work before it merges.",
    msgs: [
      {
        kind: "thinking",
        body: "This one touches the merge path — mapping the blast radius before I write anything down.",
      },
      {
        kind: "probe",
        summary: "Read 16 files across runtime, git & review · 3 touchpoints · 1 related item",
        findings: [
          {
            kind: "warn",
            text: "Merging happens in two places today: the Git panel and the workflow `merge` step. A gate has to cover both.",
          },
          {
            kind: "ok",
            text: "`PrChecks` already models a pass/fail/pending trio — a human gate can reuse that shape.",
          },
          {
            kind: "dep",
            text: "Overlaps FLT-142: reattaching a paused agent needs the persisted registry.",
          },
        ],
      },
      {
        kind: "question",
        question: {
          id: "gate-shape",
          header: "review gates · merge path",
          prompt:
            "Two shapes here, and they cost very differently. Where should the gate actually sit?",
          multiSelect: false,
          allowOther: true,
          options: [
            {
              id: "merge",
              label: "Hard gate on merge",
              desc: "The agent opens the PR, then blocks. Nothing lands without an approval. Strongest, and it needs the pause/resume work.",
              recommended: true,
            },
            {
              id: "pr",
              label: "Gate before the PR opens",
              desc: "Review the diff inside Fletch first. Cheaper, but reviewers lose GitHub's tooling.",
            },
            {
              id: "advisory",
              label: "Advisory only",
              desc: "Notify a reviewer and keep going. Ships in a day and satisfies nobody who asked for this.",
            },
          ],
        },
        answered: {
          text: "Hard gate it is. That's three items, not one — the gate, the pause/resume it depends on, and the reviewer surface. I've ordered them so nothing is blocked.",
          note: "1 epic · 3 additions · 1 dependency",
          changes: [
            {
              kind: "add",
              item: {
                code: "FLT-152",
                horizon: "now",
                size: "M",
                area: "runtime",
                source: "pm",
                status: "open",
                epic: "Review gates",
                title: "Pause an agent at the merge boundary",
                why: "The gate is only real if the agent can stop cleanly and resume later — this is the hard half.",
                accept: [
                  "Agent parks with its worktree intact",
                  "Resume replays the remaining steps",
                  "Covers the workflow merge step too",
                ],
                deps: ["FLT-142"],
              },
            },
            {
              kind: "add",
              item: {
                code: "FLT-153",
                horizon: "next",
                size: "M",
                area: "vcs",
                source: "pm",
                status: "open",
                epic: "Review gates",
                title: "Approval gate on PR merge",
                why: "Reuses the existing checks model, so the gate reads like any other check.",
                accept: [
                  "Named reviewers per project",
                  "Gate visible in the title-bar capsule",
                  "Override with an audit note",
                ],
                deps: ["FLT-152"],
              },
            },
            {
              kind: "add",
              item: {
                code: "FLT-154",
                horizon: "later",
                size: "S",
                area: "chrome",
                source: "pm",
                status: "open",
                epic: "Review gates",
                title: "Reviewer inbox",
                why: "Reviewers need one place to see what is waiting on them across projects.",
                accept: [
                  "Cross-project queue",
                  "Approve or request changes without opening the agent",
                ],
                deps: ["FLT-153"],
              },
            },
          ],
        },
      },
    ],
  },
  {
    prompt: "Agents should check with me before they touch anything in the Tauri shell.",
    msgs: [
      {
        kind: "thinking",
        body: "Sounds like one rule, but I think it's two mechanisms. Checking it against what's already planned.",
      },
      {
        kind: "probe",
        summary: "Read 11 files · 2 touchpoints · 1 conflict with the current plan",
        findings: [
          {
            kind: "warn",
            text: '"Sensitive paths" doesn\'t exist yet — there is no path policy anywhere in the runtime.',
          },
          {
            kind: "dep",
            text: "The stopping half is exactly FLT-152 (pause at a boundary), already in flight.",
          },
          {
            kind: "warn",
            text: "FLT-137 (backpressure) touches the same stream loop. Doing both at once will conflict.",
          },
        ],
      },
      {
        kind: "text",
        body: "The pause mechanism is already on the board, so this is really just the policy — which paths are sensitive, and who gets asked. I'd also pull FLT-137 forward so the two stream changes don't collide later.",
      },
      {
        kind: "proposal",
        note: "2 additions · 1 reorder",
        changes: [
          {
            kind: "add",
            item: {
              code: "FLT-155",
              horizon: "next",
              size: "S",
              area: "runtime",
              source: "pm",
              status: "open",
              title: "Sensitive path policy per project",
              why: "The rule people actually want: declare paths that require a human before an edit.",
              accept: [
                "Glob list editable in project settings",
                "Matches shown before the agent starts",
              ],
              deps: ["FLT-152"],
            },
          },
          {
            kind: "add",
            item: {
              code: "FLT-156",
              horizon: "later",
              size: "S",
              area: "chrome",
              source: "pm",
              status: "open",
              title: "Ask-before-edit prompt in the agent view",
              why: "The interruption surface — small, but it's the whole experience of the feature.",
              accept: [
                "Inline approve / deny / always-allow",
                "A denial leaves a note in the transcript",
              ],
              deps: ["FLT-155"],
            },
          },
          {
            kind: "move",
            code: "FLT-137",
            from: "next",
            to: "now",
            why: "Land the stream change before the pause work touches the same loop.",
          },
        ],
      },
    ],
  },
];

/** What the PM plays when the user says something no script beat covers: it
 *  refuses to size an unshaped idea and parks it in Later instead. */
export function freeFormBeat(text: string, code: string): PmBody[] {
  const title = text
    .replace(/^(i'?d like|i want|can you|please)\s+/i, "")
    .replace(/\.$/, "")
    .trim();
  return [
    {
      kind: "probe",
      summary: "Read 6 files · no existing capability matches this yet",
      findings: [
        {
          kind: "warn",
          text: "Nothing in the product map covers this — it would be new surface area.",
        },
      ],
    },
    {
      kind: "text",
      body: "I can't size this until we agree on the shape, so I'll park it in Later as an idea and we can shape it when you want to.",
    },
    {
      kind: "proposal",
      note: "1 addition · unshaped",
      changes: [
        {
          kind: "add",
          item: {
            code,
            horizon: "later",
            size: "M",
            area: "chrome",
            source: "pm",
            status: "open",
            title: title || "Captured idea",
            why: "Captured from your note. Needs shaping before it can be sized or ordered.",
            accept: ["Shape it with the PM agent", "Then size and place it"],
          },
        },
      ],
    },
  ];
}
