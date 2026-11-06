# Project surfaces: structure, UI and delivery plan

The project page today is one door labelled "settings" behind which live three
different kinds of thing: a place to work (Roadmap), a knowledge base (Context),
a report (Activity), and a ten-section scroll of configuration that mixes
project identity, run commands, secrets, VM policy, an app-wide integration and
automation dials. This document proposes the structure that separates them, the
UI each one gets, and the order to build it in.

## 1. What is there today

**Entry points** (all land on `ProjectScreen`, full screen, "Back to app"):

| From | Lands on |
| --- | --- |
| Sidebar project row, gear ("Project settings") | Settings |
| `⌘⇧,` and the palette's "Project settings" | Settings |
| Title-bar project pill, palette "Open project", a run's roadmap chip | Roadmap |
| Egress toast, `.fletch/settings.json` approval prompt | Settings (then scroll to Network) |

**Tabs**: Roadmap · Activity · Context (only with the developer flag
`context_layer_enabled`) · Settings.

**The Settings tab**, one scroll, in order: General (name, repositories),
Run & environment (detected commands with `ValueChip`), Environment variables
(`.env` membrane with the cube `SandboxToggle`), Sandbox (placement `<select>`),
Network (442 lines: posture `<select>`, pending approvals, allow-a-host input
with a file-target `<select>`, reached, blocked, allowed, inherited `<details>`),
Linear (an **app-wide** API key plus the project's team), Roadmap (autoqueue
toggle, concurrency `<select>`, PM-review toggle), Autopilot (toggle), Verify on
turn end (toggle), Delete project.

**Two design languages.** The global Settings screen uses `SetHead / SetGroup /
SetRow / SetToggle / SetSeg / SetDisclosure / Select / Scale`. The project page
uses its own `ps-section / ps-field / ps-label` vocabulary with raw `<select>`
and `<input>` elements and the quick-popover `Toggle`. Same app, two looks.

**Copy.** Every section opens with two to four sentences explaining the
mechanism. A recorded constraint already says a row's description is two lines
at most.

## 2. Diagnosis

1. **Three content kinds under one door.** Because the gear is the common way
   in and Settings is the tab it lands on, the whole page reads as "settings",
   and Roadmap and Context read as settings sub-features. They are not.
2. **No internal navigation in Settings.** Ten sections, one scroll, no index.
   The global screen solved this with a left nav and grouped panes; the project
   page did not borrow it.
3. **Scopes mixed.** The Linear API key is this Mac's and already has a home in
   Settings › Account; the project page duplicates it. Placement and egress
   posture are overrides of app defaults, but nothing marks them as overrides.
4. **Feature dials far from the feature.** The roadmap's autonomy dial decides
   what the board's Accept button does, yet lives under Settings. The Context
   tab leads with two toggles before any content.
5. **Generic controls where the app has specific ones.** Concurrency is a
   `<select>` of four numbers where the composer's effort picker and the VM
   knobs use `Scale`. Placement is a `<select>` of three words where `SetSeg`
   fits. The file target is a `<select>` where a two-stop segment fits.
6. **Prose where a sub-line would do.**

## 3. Target structure

### 3.1 Two project views, tabs in the header

A project has two full-screen views in the takeover slot: what the project
*is* (the Roadmap) and how it is *configured* (Settings). They share one thin
header — an icon-only back chevron ("Back to app"; Escape still closes), the
project's icon and name, the view's name muted — and, in its centre, the
view's tabs as one segmented control: a raised pill in a sunken track, each
tab with its icon. The same control everywhere, so the two views read as one
family while the tabs say which one is open.

- **Roadmap** (`RoadmapScreen/`): Board · Activity · Context. The board and
  the PM thread; what has been built; and, while the layer's developer gate is
  open, what the host knows about the project (`ProjectContext/` is the tab's
  body). The horizon counts sit at the right. The tab is remembered for the
  session; with the gate closed a Context tab shows the board.
- **Project settings** (`ProjectSettingsScreen/`): General · Run · Sandbox ·
  Network · Agents and their panes (3.2). It owns the run-config load.

The title bar's crumb names the view as a third segment, `fletch / ▣ Goreme /
Roadmap`; "fletch" closes it. The store holds one `projectScreen: { repoPath,
view }` plus the remembered tab of each view.

```
┌ ‹  ▣ Goreme / Roadmap           [▣ Board] Activity  Context           now 3 · next 5 · later 12 · shipped 41 ┐
│                                                                                                                │
│  (board, Activity, or Context)                                                                                 │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Settings: five panes, shared rows, a frame of its own

The Settings tab borrows the global screen's *row* vocabulary and nothing of
its *frame*. Rows, groups, toggles, segments, selects, scales and chips are the
`set-*` primitives, so a control behaves the same wherever it is. Everything
around them says "one project":

- **Place.** Project configuration is its own view under the shared project
  header (3.1). The left rail and the Fletch footer stay reserved for global
  settings.
- **Navigation.** Five tabs in the header's centre — General · Run · Sandbox ·
  Network · Agents — as the same segmented control the roadmap's Board ·
  Activity · Context uses (3.1). No left rail, no group headers. The pane
  below is one column at the global panes' width.
- **Who it is about.** The project's icon and name sit in the view header
  (3.1). The pane eyebrow then says only the scope,
  "Project settings · Run", against the global "Settings · Git"; it does not
  repeat the name. The H1 is the pane name in both.
- **Inherited rows.** Any row that overrides an app default shows the app's
  value as its first stop ("App default · Open") and an override dot. The row
  itself keeps saying it is the project layer over the app.
- **Words.** The sidebar, the shortcut and the palette already say "Project
  settings"; the global screen says "Settings" and "All settings". Keep that
  split everywhere copy names them.
- **Chrome classes.** The project tab gets its own `proj-set-*` chrome (tab
  row, pane column, eyebrow) and never reuses `.set-screen` or `.set-nav`, so
  the two cannot drift into looking the same by accident.

A `nav.ts` like `SettingsScreen/nav.ts` feeds both the sub-tabs and the palette
("Project settings › Run").

```
┌ ‹  ▣ Goreme / Settings                                                                                               ┐
│   General   Run   Sandbox   Network   Agents                                                                        │
│  ───────────────────────────────────────────────────────────────────────────────────────────────────────────────── │
│                                                                                                                      │
│            PROJECT SETTINGS · RUN                                                                                    │
│            Run                                                                                                       │
│            What every agent in this project runs, and with what.                                                     │
│                                                                                                                      │
│            COMMANDS                                                                 ◦ bun                            │
│            install     bun install                               Detected · package.json                             │
│            dev         bun run dev                               Project setting                                      │
│            test        bun test                                  Detected · package.json                             │
│            …                                                                                                         │
│            ENVIRONMENT VARIABLES                                                                                     │
│            Nothing from .env reaches the sandbox unless you switch it on.                                            │
│            DATABASE_URL   from .env                              [postgres://…]   [▢ cube]                           │
│            STRIPE_KEY     Overridden                             [sk_test_…]      [▣ cube]                           │
│            + Add a variable                                                                                          │
│            CHECKS                                                                                                    │
│            Run checks after each turn   Install, test and lint from above, after every turn.           [toggle]     │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Where everything goes:

| Pane › group | Row | Control | Key | Today |
| --- | --- | --- | --- | --- |
| General › Identity | Name | `TextInput`, commit on blur | `renameProject` | General |
| General › Repositories | one compact row per repo: label, path, Change…, Detach; "Attach repository…" | list | attach / detach / relocate / label | General |
| General › Issue tracker | Linear team | `Select`, "None" first; when Linear is not connected the sub links to Settings › Account | `linear.team_id`, `linear.team_name` | Linear (key UI removed) |
| General › Danger zone | Delete project | danger button, inline confirm | `deleteProject` | Delete project |
| Run › Commands | install / dev / test / lint / build … | `ConfigRow` + `ValueChip`; ecosystem badge beside the group label | `run.*` | Run & environment |
| Run › Environment variables | one row per variable | `EnvVarRow` (`ValueChip` + cube), `AddVarForm` | `run_env` + keychain | Environment variables |
| Run › Checks | Run checks after each turn | `SetToggle` | `verify.on_turn_end` | Verify on turn end |
| Sandbox › Isolation | Where new workspaces run | `SetSeg`: Automatic · Linux VM · macOS sandbox; inherited row | `sandbox.placement` | Sandbox |
| Network › Posture | New hosts | `Select`: App default · Open / Open / Ask / Allow-list; inherited row | `sandbox.egress_mode` | Network |
| Network › Needs approval | pending hosts | alert group with Allow, only while something is pending | project file approve | Network |
| Network › Hosts | Allow a host (input + Shared / Just me `SetSeg`); list with source chip and Remove | reuse `YourHosts` with project scope | `.fletch/settings*.json` | Network |
| Network › Recent activity | reached beyond the list, blocked; Allow | `SetDisclosure`, reuse `RecentHosts` | egress log | Network |
| Network › From Fletch and your list | inherited hosts, Turn off / on | `SetDisclosure`, reuse `FletchAllows` | | Network |
| Agents › Autopilot | Get PRs to mergeable automatically | `SetToggle` | `autopilot_set` | Autopilot |
| Agents › Workflows | Default workflow | `Select` | `workflow.default` | implicit (last used in the composer) |
| Agents › Project context (only with the layer enabled) | Serve context to agents · Extract decisions in the background | `SetToggle` ×2 | `context.enabled`, `context.extract` | Context tab header |
| Roadmap board (popover, see 3.3) | Queue on accept · Runs at once · PM reviews finished runs · Triage captured items | `SetToggle` · `Scale` 1–4 · `SetToggle` · `SetToggle` | `roadmap.*` | Roadmap section |

Network is its own sub-tab rather than a switch inside Sandbox as on the global
screen: on a project it is the pane the egress toast and the file-approval
prompt deep-link to, and the one with the most to show. Those deep links land
on the Network sub-tab, not on a scroll offset.

**Inherited rows.** Placement and posture (and anything later that inherits an
app default) use one row shape: the control's first stop is "App default · X",
an overridden row carries the same dot the run-config rows use, and a revert
control beside it clears the override. One `inherited` prop on `SetRow`, no
per-pane prose about "same as every project".

### 3.3 Roadmap: a surface with a pipeline

The roadmap keeps what works (horizons, ghosts, the decision strip, the
in-flight rail, the decision log) and changes three things.

**Board first.** The board takes the page; the PM thread is a panel the user
opens from the board header ("Plan with PM") and closes, with open state and
width persisted. Shaping a proposal still happens in a real chat; triaging the
board no longer gives half the window to one.

**Autonomy on the board.** The three autonomy rows move from Settings into a
popover behind an "Autonomy" control in the board header, next to the Accept
buttons whose labels they change.

**Inbox and triage.** Capture is cheap and lands untriaged; the PM turns it into
rulings.

```
┌ Roadmap ─────────────────────────────────────────────────────── [Plan with PM] [Autonomy ▾] [+] ┐
│ Capture an idea or ticket…                                                                 ⏎   │
│                                                                                                │
│ INBOX 3                                                                                        │
│  ◦ "voice note from phone: the run panel should remember the port"   triaging…                 │
│  ◦ "what if agents could pair on one checkout"            Idea → brief, 2 questions  [Open]    │
│  ◦ "thoughts after the demo (long)"                       Brainstorm → 3 proposed    [Review]  │
│                                                                                                │
│ NEEDS YOU 1 ·· IN FLIGHT 2                                                                     │
│                                                                                                │
│ NOW · committed                                                                                │
│  FLT-150 …                                                                                     │
│ NEXT · up next                                                                                 │
│  FLT-151 (ghost, from inbox #2)   [Accept] [Accept & queue] [Discard]                          │
│ LATER · someday                                                                                │
│  …                                                                                             │
│ Not doing (4)                                                                                  │
└────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Capture entry points: the board's capture field; the palette's "Add to
roadmap…"; the phone's project screen (text or dictated, see 3.5); later, a
"Send to roadmap" action on an agent message.

Triage is a PM turn the host schedules the way it schedules the settle review
today (`roadmap/review.rs`): for each captured item (batched per project) the PM
reads the item, the board, the codebase and the project context, and calls one
new op, `roadmap_triage`, with a classification and an outcome:

- **ask** — the user wants this built, however loosely worded. The captured
  row itself becomes the `proposed` ghost, with the refined title, why, area,
  acceptance criteria and deps, in the horizon the PM argues for. The code and
  history stay with it ("captured → triaged as ask → proposed → accepted").
- **idea** — worth researching before anyone writes a ticket. The row becomes
  an idea card in the Inbox with a short brief and up to three questions; any
  concrete slice the PM is already sure of is proposed as a ghost linked to it.
  The user answers in the card (which starts a PM chat on it) or dismisses.
- **brainstorm** — a session to mine. The PM proposes the few items worth
  taking, each linked to the captured row, and the row closes as "mined" with a
  note listing them.

Every outcome is a proposal the user rules on with the machinery that exists:
ghosts, the card's decision bar, the batch bar. Nothing reaches the board
unruled. A fourth autonomy row, "Triage captured items automatically" (default
on), lets a user who prefers to drive the PM by hand keep the inbox quiet.

The data change is small: a `captured` status, a nullable `kind`
(`ask | idea | brainstorm`), a nullable `triage` JSON (brief, questions), and a
`from_item` link on rows a triage produced. The PM brief
(`crates/fletch-core/src/instructions/roadmap.md`) gains one section.

### 3.4 Context: content first

The Roadmap's Context tab leads with the entities, review queue and preview. The two switches
move to Settings › Agents › Project context (shown only while the layer is
enabled); the tab header shows a status chip ("Serving to agents · extracting")
that links there. The pane is restyled on the `set-*` primitives.

### 3.5 Entry points

| Surface | Change |
| --- | --- |
| Sidebar project group | The header is the project's own row: its **name opens the Roadmap**, the project's home, and the rest of the row (chevron, count, empty space) folds the agents, the way a file tree's arrow expands while its name opens. Nothing that launches a view sits among the agent rows. The hover actions at the right are the gear ("Project settings", General) and the always-visible "+". |
| Title bar | The pill opens the roadmap. On a view the crumb reads `fletch / ▣ Goreme / Roadmap` (or Settings). |
| Palette | "Roadmap · <project>" per project, "Project settings" and "Roadmap" for the project in front, "Context · <project>" (the roadmap's Context tab) while the layer is on, and "Add to roadmap…". |
| Shortcuts | `⌘⇧R` opens the roadmap, `⌘⇧,` the settings; pressed on their own view, either leaves it. |
| Toasts and prompts | Egress toast and the project-file prompt land on the Network sub-tab. |
| Phone | The project screen gains a Roadmap segment: the Inbox and Next up as read-only lists, a capture field with the existing dictation, and the "Plan with PM" button it already has. |
| Home | Unchanged for now. A "next up" card is a later option once the inbox exists. |

### 3.6 Project identity: icons

Projects are told apart by an icon, not a hue. The icon is the project's own
where one can be found, a neutral mark where it cannot, and the user's choice
whenever they want.

**Where it shows, at what size.** Sidebar project row before the name (14px),
project page header (16px), title bar crumb and pill (12px), palette project
rows and the "New agent in…" picker (16px, a new `image` leading kind beside
`icon` and `swatch`), the draft header, and the phone's project list and
project screen, where it replaces the hued swatch. One `ProjectIcon` component
draws all of them: the image when there is one, else the mark. Every placement
is the same silhouette, a rounded square, so a project looks the same in each.

**The generic mark.** A rounded square with the project's initial, monochrome.
Not an ecosystem glyph: three web projects as three globes are indistinguishable
in a sidebar, three initials are not. A small built-in glyph set (web, desktop,
mobile, CLI, library, docs) is offered in the picker for a user who prefers one.

**Detection** is a host-side scan, deterministic and free, run when a project is
added and on "Detect again". It reads the project's repository (never a
checkout), shallowly (depth three, skipping `node_modules`, `target`, `.git`,
`dist`), the primary repo first in a multi-repo project, and ranks what it
finds:

1. App icons the build names: `src-tauri/icons/` and `tauri.conf.json`'s
   `bundle.icon`; Electron's `build/icon.*` and the builder config's `icon`;
   `*.xcassets/AppIcon.appiconset` (largest entry); Android `mipmap-*/ic_launcher*`;
   Flutter's launcher-icon config.
2. Web manifest icons (largest), Next's `app/icon.*`, `<link rel="icon">` in
   `index.html`.
3. `favicon.svg` / `.png`, `apple-touch-icon.png`, then `favicon.ico`.
4. `logo.*` in `public/`, `assets/`, `static/`, `docs/`, and the first local
   image a `README.md` references.

SVG and PNG are preferred; ICO is taken only when nothing else exists, and only
the largest embedded PNG entry. The scan returns candidates, not a verdict: the
top one becomes the icon, the rest wait in the picker. An agent pass is a later
option for a repository the scan draws a blank on: a read-only one-shot turn,
as the handoff summarizer runs, asked for the path of the project's logo, with
the answer validated as an existing image before it counts. It is not in the
first cut, and never runs on its own.

**Customization** is one control in Settings › General › Identity, beside the
name: the icon as a button that opens a popover with the detected candidates as
thumbnails captioned by path, "Choose file…", the built-in glyphs, "Use
initial" and "Detect again". No inputs. On a paired host the file choice goes
through the host browser, which today picks folders only; that pane shows
candidates and glyphs until the browser can pick a file.

**Storage.** A project setting, `icon.source`, records the choice
(`detected:<repo-relative path>`, `file`, `builtin:<glyph>`, or absent for the
initial). The image itself is normalised once, to a 128px PNG or the SVG as is,
and kept under the host's data directory keyed by project id, so rendering
never reads the repository, a relocated repo keeps its icon, and the phone
fetches it with one op by hash. `ProjectRef` gains `icon_hash`, so every client
knows when to refetch without the bytes riding on each snapshot. SVGs are drawn
through `<img>`, never inlined, so nothing in a repository's SVG can run.

## 4. Control and copy rules

**One primitive set.** Project settings use `SetHead / SetGroup / SetRow /
SetToggle / SetSeg / SetDisclosure / Select / Scale / TextInput` and the shared
`ValueChip` and `SandboxToggle`. The `ps-*` form vocabulary goes away; what
remains of `ProjectScreen.css` is the page chrome and the roadmap.

**Which control for which shape:**

| Shape | Control | Example |
| --- | --- | --- |
| On / off | `SetToggle` | Autopilot, checks after each turn |
| 2–3 named stops | `SetSeg` | placement, Shared / Just me |
| An ordered scale of a few stops | `Scale` | runs at once (1·2·3·4) |
| A list of named things | `Select` with hints | posture, Linear team, default workflow |
| A free value with a detected default | `ValueChip` | run commands, env values |
| An escape hatch few people need | `SetDisclosure` | inherited hosts, recent egress activity |

**Copy.** A pane description is one line naming the category. A row's sub is at
most two lines and says what the row does, not how it is implemented. Mechanism
goes in a tooltip on the control or in docs. Examples:

| Today | Proposed |
| --- | --- |
| "When on, every agent in this project keeps its open PR moving: it fixes failing checks, resolves conflicts, updates the branch and answers review comments without being asked. Anything else — committing, opening the PR, merging — stays yours. On by default." | **Autopilot** — Fixes failing checks, conflicts and review comments on open PRs. Never commits, opens or merges. |
| "Accepted items queue automatically" + a four-line paragraph | **Queue on accept** — Accepting a proposal sends it straight to the queue. |
| "Runs at once" + a three-line paragraph | **Runs at once** `1·2·3·4` — Parallel runs open parallel PRs into one repo. |
| "Variables from this project's .env — nothing reaches the sandbox unless you switch it on. Edit a shared value to override it (use {{agent_id}} per agent), or add one below." | Group sub: Nothing from `.env` reaches the sandbox unless you switch it on. The `{{agent_id}}` hint moves to the chip's tooltip. |
| "What this project's box agents may reach: Fletch's list and your changes in Settings › Sandbox › Network, then this project's .fletch/settings.json (commit it to share with your team) and .fletch/settings.local.json (just you)." | Pane desc: What this project's VM agents may reach. The file semantics live on the Shared / Just me control's tooltip. |

## 5. Delivery plan

Eight pull requests. 1 → 2 → 3 → 4 → 5 is the spine; 6 needs 1; 7 needs 1 and
3; 8 (icons) needs 1 and is best done right after it, since identity is what
makes the page read as one project's. PR 3 can run alongside PR 1 if it deletes
the roadmap rows from wherever they then live.

### PR 1 — Project settings on the app's row primitives, in its own frame

Restructure only: five sub-tabs, every knob where section 3.2 puts it, writing
the same keys as today. Rows use the `set-*` primitives; the frame (identity in
the header, sub-tab row, single-column pane, "Project settings · X" eyebrow) is
the project page's own `proj-set-*` chrome. Controls swap 1:1 where trivial
(`Toggle` → `SetToggle`, `<select>` → `Select`); new control shapes wait for
PR 2. The roadmap autonomy rows sit under Agents until PR 3 moves them. The
header's icon is the initial mark only; detection and customization are PR 8.

- New: `src/components/ProjectIcon/index.tsx` (image or initial mark, every size), `src/components/ProjectScreen/Settings/{index.tsx, nav.ts, GeneralPane.tsx, RunPane.tsx, SandboxPane.tsx, NetworkPane.tsx, AgentsPane.tsx}`.
- Folded in and deleted: `GeneralSection`, `RepositoriesField` (moves under General), `DeleteSection`, `RunEnvSection`, `EnvVarsSection/` (moves under Run), `SandboxSection`, `NetworkSection`, `LinearSection` (team only; the key UI is removed in favour of a link to Settings › Account), `AutopilotSection`, `VerifySection`, `RoadmapSection`.
- Changed: `ProjectScreen/index.tsx`, `ProjectHeader.tsx` (icon-only back, icon and name at left, Settings last, divider), `TitleBar/WorkspaceStatus/index.tsx` (the project crumb on the `fletch / name` pattern), `ProjectScreen.css` (sub-tab row, pane column and eyebrow as `proj-set-*`; row styling comes from the shared `set-*` row classes, never `.set-screen` / `.set-nav`), `store/ui.ts` (`projectSettingsSection`; `openProjectScreen(repo, tab?, section?)`), callers `Sidebar/ProjectGroup.tsx`, `BoxEgressToast.tsx`, `ProjectFilePrompts.tsx`, `CommandPalette/commands.ts`, `CommandPalette/entities.ts`, `util/shortcuts.ts`.
- Acceptance: every setting reachable in at most two clicks from the sub-tabs; each writes the key it wrote before (`hostOwnedKeys.test.ts` still passes); the egress toast and the project-file prompt open the Network sub-tab; the palette lists the five panes; the header shows the project's icon and name on every tab, at the same height as today; no `ps-field` / `ps-label` markup remains in the settings tab; a screenshot of a project pane beside the global Git pane reads as two different places.

### PR 2 — Controls, inheritance and copy

- `SetRow` gains an `inherited` prop (default label, override dot, revert); placement becomes a `SetSeg`, posture a `Select` on an inherited row, runs-at-once a `Scale`, the file target a `SetSeg`.
- `SettingsScreen/Sandbox/Network/{PostureRow, YourHosts, RecentHosts, FletchAllows}.tsx` take a scope (global or project) so the project Network pane is a composition of them; the project-only parts (pending approvals, file rows) stay in `NetworkPane.tsx`.
- Copy across the four panes trimmed to the rules in section 4; `util/egressMode.ts` and `util/sandboxNames.ts` carry the short labels.
- Acceptance: no raw `<select>` or `<input>` in project settings; an overridden placement or posture shows the dot and reverts to the app default in one click; every row sub fits two lines at the pane's width; network edits still write the chosen file; Linear's key is set only in Settings › Account.

### PR 3 — Roadmap: board first, autonomy on the board, capture field

- `Roadmap/index.tsx`: the thread becomes a toggleable panel; `roadmapThreadOpen` joins the persisted ui slice beside `roadmapBoardWidth`.
- New `Roadmap/Board/BoardHeader.tsx` (Plan with PM, capture field, Autonomy, +) and `Roadmap/AutonomyPopover.tsx` (the three rows, `Scale` for concurrency); the rows leave Settings › Agents.
- The capture field creates a plain `open` / `later` item for now (it becomes `captured` in PR 4).
- Acceptance: the board can take the full width and the thread state survives a relaunch; the popover writes `roadmap.autoqueue`, `roadmap.max_concurrent`, `roadmap.settle_review` and the board's Accept labels follow it; Enter in the capture field adds a card without a dialog.

### PR 4 — Inbox and triage

- Host (`crates/fletch-core/src/roadmap/`): migration for `captured`, `kind`, `triage`, `from_item`; `roadmap_capture` client op; `roadmap_triage` PM op with the three outcomes; a triage scheduler modelled on `review.rs`; `roadmap.triage` autonomy key in `autonomy.rs`; the brief's new section in `instructions/roadmap.md`.
- Client: `api/types/roadmap.ts`, `api/domains/roadmap.ts`, `Roadmap/Board/InboxLane.tsx` and `InboxCard.tsx`, a fourth row in the autonomy popover, the palette's "Add to roadmap…".
- Acceptance: a capture from the palette appears in the Inbox at once; an ask becomes a ghost with code and history intact; an idea shows a brief and questions and opens a PM chat on it; a brainstorm yields linked ghosts and closes as mined; with triage off nothing runs until the user opens the item in a chat; the drainer never dispatches a `captured` row.

### PR 5 — Capture from the phone

- `mobile/src/screens/Project`: a Roadmap segment with the Inbox and Next up as read-only lists, a capture field on `PromptField` (dictation included), and the existing Plan with PM action.
- Acceptance: a dictated note lands in the Inbox on the Mac within a second of send; the lists follow `roadmap:item` events.

### PR 6 — Context, content first

- `ProjectContext/index.tsx`: the switches move to Settings › Agents (gated); the header becomes a status chip and the area nav; styling on `set-*`.
- Acceptance: the tab opens on the entity list; the chip links to the two rows; the rows are hidden while the layer is disabled.

### PR 7 — Entry points and polish

- Sidebar icon "Open project" on the last-used tab; `projectScreenTab` remembered; the tab divider and, optionally, a project eyebrow in the header; a Home card once the inbox is live.
- Acceptance: the gear path still reaches Settings › General through `⌘⇧,` and the palette; opening the page twice returns to the tab left open.

### PR 8 — Project icons (recommended straight after PR 1)

- Host (`crates/fletch-core`): an `icon_detect` module beside `run_detect` with the ranked scan of 3.6; normalisation to a 128px PNG (SVG kept) under the data directory; `project_icon_candidates`, `project_icon_set` and `project_icon_get` ops; `icon_hash` on `ProjectRef`; the `icon.source` project-settings key in the allowlist; a scan on project add and relocate.
- Client: `ProjectIcon` learns to fetch and cache by hash; placements in `Sidebar/ProjectGroup.tsx`, `TitleBar/WorkspaceStatus/{ProjectPill,index}.tsx`, `CommandPalette/{PaletteRow,entities}.tsx` (`image` leading kind), the new-agent picker, `Workspace/EmptyWorkspace.tsx`; the Identity row's picker popover in `Settings/GeneralPane.tsx`; `storage/hostOwnedKeys.ts`.
- Phone: `components/ui` `Swatch` becomes the shared `ProjectIcon` with the initial fallback, monochrome; fetch by hash on the project list.
- Acceptance: a Tauri, a Next and a Vite repo each show their own icon in the sidebar within a second of being added, with no agent involved; a repo with no icon shows its initial; the picker lists every candidate with its path, and "Choose file…" and "Use initial" round-trip; relocating a repo keeps its icon; the phone shows the same icon as the Mac; an SVG with a script element renders without executing it.

## 6. Open questions

- Should the scan also look at a GitHub origin's organisation avatar as a last resort? It is the owner's picture, not the project's, and needs the network; this plan leaves it out.
- Does the phone need the icon bytes in the handshake snapshot, or is a fetch-by-hash on first paint fine? Fetch-by-hash is the plan; the initial mark covers the first frame.
- Should "Run checks after each turn" sit under Run › Checks (next to the commands it runs) or under Agents (next to the other things agents do on their own)? This plan says Run; either is defensible.
- Does triage run one PM turn per captured item or one per batch? Per batch is cheaper and lets the PM spot duplicates; per item is simpler to show progress for. Start per batch with a short debounce.
