// In-memory fake host. It is a `SocketFactory`, so it speaks the same JSON
// frames over the same client as a real host — nothing above the socket knows
// the difference, and the pairing/reconnect/envelope code is exercised for
// real in mock mode and in tests.

import type { AgentRecord, Workspace } from "@desktop/api/types/agent";
import type { DirEntry, DirListing } from "@desktop/api/types/checkout";
import type { RoadmapItem, RoadmapItemPatch } from "@desktop/api/types/roadmap";
import type { SessionRecord, UserTurn } from "@desktop/api/types/session";
import type { Socket, SocketFactory } from "@desktop/remote/socket";
import {
  CLOSE_BAD_FIRST_FRAME,
  CLOSE_UNAUTHENTICATED,
  type EventFrame,
  type RequestFrame,
  type ResponseFrame,
} from "@desktop/remote/types";
import { parseRepoSpec } from "@desktop/util/repoSpec";
import { baseName, childPath, parentPath } from "../../lib/paths";
import * as fx from "./fixtures";
import { scriptFor } from "./script";

/** The mock host has no Noise handshake to run, so it reports a fixed
 *  identity: the client pins it exactly as it would a real one. */
export const MOCK_HOST_KEY = "mock-host-key";
const MOCK_PAIRING_TOKEN_LEN = 8;

export interface MockOptions {
  /** Wall-clock scale for the scripted stream; 0 emits everything at once
   *  (what the tests use). */
  speed?: number;
}

interface MockState {
  workspace: Workspace;
  records: Record<string, SessionRecord[]>;
  /** Fletch-origin metadata per sent turn (`session_user_turns`), which the
   *  phone overlays on the rebuilt transcript — it is where a rebuilt bubble
   *  gets its attachments and typed text back from. */
  turns: Record<string, UserTurn[]>;
  /** The current turn's live frames per agent, as the host buffers them for
   *  `read_live_turn`: cleared when a turn starts, never at its end. */
  liveTurns: Record<string, Record<string, unknown>[]>;
  pendingToolUse: Record<string, string>;
  /** The roadmap boards, flat across projects as the host's table is. */
  roadmapItems: RoadmapItem[];
  nextPrNumber: number;
  /** Mutable, because cloning creates a directory the next clone must trip
   *  over ("a folder already exists at …"). */
  filesystem: Record<string, DirEntry[]>;
}

/** Tilde expansion, as the host does it — including the trailing slash a bare
 *  `~` comes back with, which is exactly the shape `childPath` has to collapse. */
function expandTilde(path: string): string {
  if (path === "~") return `${fx.HOME}/`;
  if (path.startsWith("~/")) return `${fx.HOME}${path.slice(1)}`;
  return path;
}

/** Trailing separators off (but never the root's), so one directory has one key
 *  in the fake filesystem however it was reached. */
const normalize = (path: string) => path.replace(/(.)\/+$/, "$1");

/** A live mock host session. Exposed so tests can await the scripted stream. */
export class MockHost {
  private authed = false;
  private firstFrame = true;
  private timers = new Set<ReturnType<typeof setTimeout>>();
  private state: MockState;
  private seq = 1000;
  /** Open dictation sessions → chunks received. */
  private dictation = new Map<string, number>();
  /** Open attachment uploads → the name they will be staged under. */
  private uploads = new Map<string, string>();

  constructor(
    private readonly emit: (frame: ResponseFrame | EventFrame) => void,
    private readonly opts: MockOptions = {},
  ) {
    this.state = {
      workspace: structuredClone(fx.workspace),
      records: structuredClone(fx.records),
      turns: structuredClone(fx.userTurns),
      liveTurns: {},
      pendingToolUse: { pamukkale: fx.PENDING_REQUEST_ID },
      roadmapItems: structuredClone(fx.roadmapItems),
      nextPrNumber: 649,
      filesystem: structuredClone(fx.filesystem),
    };
  }

  close() {
    for (const t of this.timers) clearTimeout(t);
    this.timers.clear();
  }

  receive(text: string) {
    let frame: RequestFrame;
    try {
      frame = JSON.parse(text) as RequestFrame;
    } catch {
      return;
    }
    const first = this.firstFrame;
    this.firstFrame = false;
    if (first && frame.op !== "pair" && frame.op !== "hello") {
      this.emit({ id: frame.id, ok: false, error: "bad first frame" });
      this.onClose?.(CLOSE_BAD_FIRST_FRAME);
      return;
    }
    if (!this.authed && frame.op !== "pair" && frame.op !== "hello") {
      this.onClose?.(CLOSE_UNAUTHENTICATED);
      return;
    }
    try {
      const result = this.dispatch(frame.op, frame.args ?? {});
      this.emit({ id: frame.id, ok: true, result });
    } catch (e) {
      this.emit({ id: frame.id, ok: false, error: e instanceof Error ? e.message : String(e) });
    }
  }

  /** Set by the socket wrapper so the host can hang up (auth close codes). */
  onClose?: (code: number) => void;

  private later(fn: () => void, ms: number) {
    const scale = this.opts.speed ?? 1;
    if (scale === 0) {
      fn();
      return;
    }
    const t = setTimeout(() => {
      this.timers.delete(t);
      fn();
    }, ms * scale);
    this.timers.add(t);
  }

  private event(event: string, payload: unknown) {
    this.emit({ event, payload });
  }

  /** The snapshot as the host reports it: purpose-tagged chats are held back,
   *  exactly as the real `get_workspace` does — they are listed on their own
   *  through `list_project_chats`. */
  private visibleWorkspace(): Workspace {
    return {
      ...this.state.workspace,
      agents: this.state.workspace.agents.filter((a) => !a.purpose),
    };
  }

  private agent(id: string): AgentRecord {
    const found = this.state.workspace.agents.find((a) => a.id === id);
    if (!found) throw new Error("agent not found");
    return found;
  }

  private patchAgent(id: string, patch: Partial<AgentRecord>) {
    this.state.workspace = {
      ...this.state.workspace,
      agents: this.state.workspace.agents.map((a) => (a.id === id ? { ...a, ...patch } : a)),
    };
  }

  private setStatus(id: string, status: AgentRecord["status"], lastError: string | null = null) {
    this.patchAgent(id, { status, last_error: lastError });
    this.event("agent:status", { agent_id: id, status, last_error: lastError });
  }

  private appendRecord(id: string, provider: string, body: Record<string, unknown>) {
    this.seq += 1;
    const list = this.state.records[id] ?? [];
    this.state.records[id] = [
      ...list,
      {
        seq: this.seq,
        provider,
        source: "transcript",
        native_id: `n${this.seq}`,
        agent_version: null,
        body,
      },
    ];
  }

  /** A directory exists if it has a row of its own or its parent names it as
   *  one — so the fixture table only has to list the interesting nodes. */
  private isDir(abs: string): boolean {
    const path = normalize(abs);
    if (path === "/") return true;
    if (this.state.filesystem[path]) return true;
    const parent = parentPath(path);
    if (!parent) return false;
    return !!this.state.filesystem[parent]?.some((e) => e.name === baseName(path) && e.is_dir);
  }

  private listDir(path: string): DirListing {
    const base = expandTilde(path);
    if (!this.isDir(base)) throw new Error(`no such directory: ${base}`);
    return { base, entries: this.state.filesystem[normalize(base)] ?? [] };
  }

  /** Track a folder as a project, the way both add-project ops end. Mirrors
   *  the host: a folder that is already a project comes back unchanged (the
   *  desktop pin is idempotent), and a folder inside an existing repository is
   *  refused with the host's own wording. */
  private addProject(path: string): Workspace {
    const repoPath = normalize(path);
    if (this.state.workspace.projects.some((p) => p.path === repoPath)) {
      return this.state.workspace;
    }
    const enclosing = this.state.workspace.projects.find((p) => repoPath.startsWith(`${p.path}/`));
    if (enclosing) {
      throw new Error(
        `${repoPath} is inside the git repository at ${enclosing.path} — add that folder instead`,
      );
    }
    const name = baseName(repoPath);
    this.state.workspace = {
      ...this.state.workspace,
      repos: [...this.state.workspace.repos, repoPath],
      projects: [
        ...this.state.workspace.projects,
        { path: repoPath, name, project_id: `prj-${name}`, label: null },
      ],
    };
    return this.state.workspace;
  }

  /** Play a scripted turn: live `agent:event` frames, then the canonical
   *  records + `session:records-appended`, exactly as a real host does. */
  private runTurn(id: string, prompt: string) {
    // A turn queued behind a discard has nothing left to run on: the host drops
    // the session with the record, it does not raise an error at nobody.
    const agent = this.state.workspace.agents.find((a) => a.id === id);
    if (!agent) return;
    const provider = agent.provider;
    const steps = scriptFor(provider, prompt);
    // The turn ends at its last persisted step. Anything after is the
    // background tail — sub-agent frames a real host keeps forwarding once the
    // agent has gone idle — and lands with no record and no status change.
    let end = steps.length - 1;
    while (end > 0 && !steps[end].record) end -= 1;
    this.event("turn:started", { agent_id: id, started_at: Date.now() });
    this.setStatus(id, "running");
    this.state.liveTurns[id] = [];
    steps.forEach((step, i) => {
      this.later(
        () => {
          this.state.liveTurns[id] = [...(this.state.liveTurns[id] ?? []), step.live];
          this.event("agent:event", { agent_id: id, event: step.live });
          if (step.record) this.appendRecord(id, provider, step.record);
          if (i === end) {
            this.event("session:records-appended", { agent_id: id });
            this.setStatus(id, "idle");
          }
        },
        700 * (i + 1),
      );
    });
  }

  // One flat table mirroring docs/remote-protocol.md's op allowlist; splitting
  // it by domain would only hide the mapping.
  private dispatch(op: string, args: Record<string, unknown>): unknown {
    const id = String(args.agentId ?? "");
    switch (op) {
      case "pair": {
        const token = String((args.token as string) ?? "");
        if (token.length !== MOCK_PAIRING_TOKEN_LEN) throw new Error("invalid pairing token");
        this.authed = true;
        this.later(() => this.bootstrap(), 400);
        return { deviceId: "mock-device", host: fx.hostInfo, protocol: fx.protocol };
      }
      case "hello": {
        // Device authentication is the handshake, which the mock socket
        // stands in for: anything that gets this far is a known device.
        this.authed = true;
        this.later(() => this.bootstrap(), 400);
        return { host: fx.hostInfo, workspace: this.visibleWorkspace(), protocol: fx.protocol };
      }
      case "get_workspace":
        return this.visibleWorkspace();
      // Read off the whole agent list rather than the visible one: this op is
      // how a phone resolves a record nothing has listed for it, purpose-tagged
      // chats very much included.
      case "get_agent":
        return this.state.workspace.agents.find((a) => a.id === id) ?? null;
      case "list_project_chats":
        return this.state.workspace.agents
          .filter((a) => a.project_id === args.projectId && a.purpose === args.purpose)
          .sort((a, b) => b.created_at.localeCompare(a.created_at));
      case "list_custom_agents":
        return fx.customAgents;
      case "roadmap_list_items":
        return this.state.roadmapItems.filter((i) => i.project_id === args.projectId);
      case "roadmap_update_item": {
        const itemId = String(args.id ?? "");
        const item = this.state.roadmapItems.find((i) => i.id === itemId);
        if (!item) throw new Error("item not found");
        // The host's conditional transition: an expectation that misses changes
        // nothing and reports the row as it really is.
        const expect = args.expectStatus ?? null;
        if (expect && item.status !== expect) return { applied: false, item };
        const patch = (args.patch as RoadmapItemPatch) ?? {};
        // Where an accept lands is the host's call — the project's autoqueue
        // dial, a board hold. The mock's call is the simple one: `queue` sends
        // an admitted row straight on to the drainer.
        const status = args.queue === true && patch.status === "open" ? "queued" : patch.status;
        const next = { ...item, ...patch, status: status ?? item.status, updated_at: Date.now() };
        this.state.roadmapItems = this.state.roadmapItems.map((i) => (i.id === itemId ? next : i));
        this.event("roadmap:item", next);
        return { applied: true, item: next };
      }
      case "roadmap_discard_proposal": {
        const itemId = String(args.id ?? "");
        const item = this.state.roadmapItems.find((i) => i.id === itemId);
        // Gone already: nothing was deleted and there is no row to report.
        if (!item) return { applied: false, item: null };
        // The host's condition: a row that has been ruled on is not a proposal
        // any more, so it survives the discard and comes back as it really is.
        if (item.status !== "proposed") return { applied: false, item };
        this.state.roadmapItems = this.state.roadmapItems.filter((i) => i.id !== itemId);
        // The payload is the bare id, as the host forwards it.
        this.event("roadmap:item-deleted", itemId);
        return { applied: true, item: null };
      }
      case "allocate_draft_name": {
        const used = new Set([
          ...this.state.workspace.agents.map((a) => a.name),
          ...((args.drafts as string[]) ?? []),
        ]);
        return (
          ["tasman", "skye", "lofoten", "zermatt", "sedona"].find((n) => !used.has(n)) ?? "kyoto"
        );
      }
      case "spawn_agent": {
        const name = String(args.name ?? "agent");
        const repoPath = String(args.repoPath ?? fx.FLETCH_REPO);
        const project = this.state.workspace.projects.find((p) => p.path === repoPath);
        const record: AgentRecord = {
          id: name,
          project_id: project?.project_id ?? "prj-fletch",
          name,
          provider: String(args.provider ?? "claude"),
          repos: [
            {
              repo_path: repoPath,
              subdir: repoPath.split("/").pop() ?? "repo",
              branch: `feat/${name}`,
              parent_branch: String(args.forkBase ?? "main"),
              label: null,
            },
          ],
          task: "",
          status: "spawning",
          view: "custom",
          session_id: null,
          created_at: new Date().toISOString(),
          last_error: null,
          archive: null,
          effort: (args.effort as string) ?? null,
          model: (args.model as string) ?? null,
          custom_agent_id: (args.customAgentId as string) ?? null,
          sandbox_engine: "sandbox-exec",
          issue_ref: null,
          // A tagged workspace is a purpose chat: it stays out of the snapshot
          // from here on, as it does on a real host.
          purpose: (args.purpose as string) ?? null,
        };
        this.state.workspace = {
          ...this.state.workspace,
          agents: [record, ...this.state.workspace.agents],
        };
        this.state.records[name] = [];
        this.state.turns[name] = [];
        // The phone waits for `spawning` to clear before sending the prompt.
        this.later(() => this.setStatus(name, "idle"), 900);
        return record;
      }
      case "send_user_message": {
        const text = String(args.text ?? "");
        const attachments = Array.isArray(args.attachments) ? (args.attachments as string[]) : [];
        // Echoed to every client before delivery, as the host does; the sender
        // recognizes its own turn id and does not draw the bubble twice.
        this.event("turn:sent", {
          agent_id: id,
          turn_id: String(args.turnId ?? ""),
          text,
          attachments,
          follow_up: this.agent(id).status === "running",
        });
        // The transcript holds what the runner sent: the text, padded with a
        // reference line per attachment.
        const sent = [text, ...attachments.map((p) => `Attached file: ${p}`)]
          .filter(Boolean)
          .join("\n");
        this.appendRecord(id, this.agent(id).provider, {
          type: "user",
          message: { role: "user", content: [{ type: "text", text: sent }] },
        });
        // The turn row, matched to the record it just wrote — as the host's
        // turn-end ingest stamps it — so the rebuilt bubble reads like the
        // optimistic one: the typed text, with the attachments hung on it.
        this.state.turns[id] = [
          ...(this.state.turns[id] ?? []),
          {
            turn_id: String(args.turnId ?? ""),
            seq: this.seq,
            text,
            attachments,
            native_id: `n${this.seq}`,
            started_at: Date.now(),
            ended_at: null,
          },
        ];
        this.patchAgent(id, { task: this.agent(id).task || text });
        this.event("session:records-appended", { agent_id: id });
        this.later(() => this.runTurn(id, text), 500);
        return false;
      }
      case "answer_tool_use": {
        const behavior = String(args.behavior ?? "allow");
        delete this.state.pendingToolUse[id];
        // As the host: a settled prompt is not replayed.
        this.state.liveTurns[id] = (this.state.liveTurns[id] ?? []).filter(
          (ev) => !(ev.type === "control_request" && ev.request_id === args.requestId),
        );
        this.appendRecord(id, this.agent(id).provider, {
          type: "user",
          message: {
            role: "user",
            content: [
              {
                type: "tool_result",
                tool_use_id: fx.PENDING_TOOL_USE_ID,
                content:
                  behavior === "allow"
                    ? "To github.com:fwdai/fletch.git\n + 3f1c9a2...8a91c04 fix/onboarding-flicker -> fix/onboarding-flicker (forced update)"
                    : "Denied by the user",
                is_error: behavior !== "allow",
              },
            ],
          },
        });
        this.event("session:records-appended", { agent_id: id });
        if (behavior === "allow") this.later(() => this.runTurn(id, "publish"), 400);
        else this.later(() => this.setStatus(id, "idle"), 200);
        return null;
      }
      // Nothing on this host is really blocked on an approval, so the answer is
      // simply accepted — the point of the arm is that the op exists here as it
      // does on a current Mac.
      case "answer_publish_approval":
        return null;
      case "stop_agent":
        this.close();
        this.setStatus(id, "stopped");
        return null;
      case "resume_agent":
        this.later(() => this.runTurn(id, "resume"), 200);
        return null;
      case "archive_agent":
        this.state.workspace = {
          ...this.state.workspace,
          agents: this.state.workspace.agents.filter((a) => a.id !== id),
        };
        this.event("workspace:changed", null);
        return null;
      // The destructive twin: the record goes for good, so a purpose-tagged
      // chat drops out of `list_project_chats` the same way a sidebar agent
      // drops out of the snapshot.
      case "discard_agent":
        this.state.workspace = {
          ...this.state.workspace,
          agents: this.state.workspace.agents.filter((a) => a.id !== id),
        };
        delete this.state.records[id];
        this.event("workspace:changed", null);
        return null;
      case "set_agent_model":
        this.patchAgent(id, { model: (args.model as string) ?? null });
        this.event("agent:model", { agent_id: id, model: args.model ?? null });
        return null;
      case "set_agent_effort":
        this.patchAgent(id, { effort: (args.effort as string) ?? null });
        this.event("agent:effort", { agent_id: id, effort: args.effort ?? null });
        return null;
      case "read_session_records":
        return this.state.records[id] ?? [];
      case "read_user_turns":
        return this.state.turns[id] ?? [];
      case "sync_session":
        return null;
      case "read_live_turn":
        return { events: this.state.liveTurns[id] ?? [], dropped: 0 };
      case "get_git_state":
        return fx.gitStates[id] ?? null;
      // Working-tree stats for the whole fleet, as the host reports them: an
      // agent whose tree is clean contributes nothing to count, so it is simply
      // absent from the map.
      case "get_all_shortstats":
        return Object.fromEntries(
          Object.entries(fx.gitStates)
            .filter(([, git]) => git.files.length > 0)
            .map(([agentId, git]) => [
              agentId,
              {
                additions: git.additions,
                deletions: git.deletions,
                file_count: git.files.length,
              },
            ]),
        );
      case "list_checkout_tree":
        return fx.checkoutTree;
      case "read_checkout_file":
        return fx.fileContents(String(args.path ?? ""));
      case "get_file_diff":
        return fx.fileDiff(String(args.path ?? ""));
      case "commit_agent":
        this.event("agent:git-action", { agent_id: id, op: "commit" });
        return null;
      case "push_agent":
        this.event("agent:git-action", { agent_id: id, op: "push" });
        return "https://github.com/fwdai/fletch/commit/8a91c04";
      case "create_pr": {
        const number = this.state.nextPrNumber;
        this.state.nextPrNumber += 1;
        const pr = {
          number,
          url: `https://github.com/fwdai/fletch/pull/${number}`,
          state: "open" as const,
          title: String(args.title ?? "Untitled"),
          mergeable: "unknown" as const,
        };
        fx.prStates[id] = pr;
        fx.prChecks[id] = {
          merge_state: "unknown",
          rollup: "pending",
          total: 11,
          passed: 0,
          failed: 0,
          pending: 11,
          required_failing: [],
          runs: [],
        };
        this.event("agent:git-action", { agent_id: id, op: "pr" });
        this.event("pr:state_changed", { agent_id: id, state: pr });
        return pr;
      }
      case "get_pr_state":
        return fx.prStates[id] ?? null;
      case "get_pr_checks":
        return fx.prChecks[id] ?? null;
      case "get_pr_live": {
        const state = fx.prStates[id];
        return state ? { state, checks: fx.prChecks[id] ?? null } : null;
      }
      case "list_repo_branches":
        return fx.branches[String(args.repoPath ?? "")] ?? ["main"];
      case "repo_default_branch":
        return "main";
      case "discover_supported_models":
        return fx.supportedModels;
      // There is no APNs behind a browser, so the mock host only has to accept
      // the op — a registration that errored would surface as a real failure.
      case "register_push":
        return null;
      // Dictation: the browser has a mic but no Mac to transcribe on, so the
      // mock counts chunks and answers a fixed sentence for any session that
      // sent audio — enough to exercise the composer's listening → transcribing
      // → text path.
      case "dictation_status":
        return { available: true, reason: null };
      case "dictation_begin": {
        const session = `dict-${this.dictation.size + 1}`;
        this.dictation.set(session, 0);
        // The Mac's default. Flip it to exercise the tap-to-stop path.
        return { session, auto_stop: true };
      }
      case "dictation_audio": {
        const session = String(args.session ?? "");
        const chunks = this.dictation.get(session);
        if (chunks === undefined) throw new Error("dictation: unknown session");
        this.dictation.set(session, chunks + 1);
        return null;
      }
      case "dictation_end": {
        const session = String(args.session ?? "");
        const chunks = this.dictation.get(session);
        if (chunks === undefined) throw new Error("dictation: unknown session");
        this.dictation.delete(session);
        return { text: chunks > 0 ? "Add retries to the upload path and log each attempt" : "" };
      }
      case "dictation_cancel":
        this.dictation.delete(String(args.session ?? ""));
        return null;
      // Attachments: no disk behind a browser, so the mock only keeps the
      // name and hands back the path the real host would stage it at —
      // enough to exercise pick → chips → send with paths on the message.
      case "attachment_begin": {
        const upload = `up-${this.uploads.size + 1}`;
        this.uploads.set(upload, baseName(String(args.name ?? "attachment")));
        return { upload };
      }
      case "attachment_chunk": {
        if (!this.uploads.has(String(args.upload ?? ""))) {
          throw new Error("attachment: unknown upload");
        }
        return null;
      }
      case "attachment_end": {
        const upload = String(args.upload ?? "");
        const name = this.uploads.get(upload);
        if (name === undefined) throw new Error("attachment: unknown upload");
        this.uploads.delete(upload);
        return {
          path: `${fx.HOME}/Library/Application Support/sh.fletch.app/attachments/${upload}/${name}`,
        };
      }
      case "attachment_cancel":
        this.uploads.delete(String(args.upload ?? ""));
        return null;
      case "list_dir":
        return this.listDir(String(args.path ?? "~"));
      case "add_workspace_repo": {
        const repoPath = expandTilde(String(args.repoPath ?? ""));
        if (!this.isDir(repoPath)) throw new Error(`no such directory: ${repoPath}`);
        // A folder that is not a repo yet gets `git init` on the real host, so
        // there is nothing to refuse here.
        return this.addProject(repoPath);
      }
      case "clone_repo": {
        const spec = String(args.spec ?? "");
        const destParent = expandTilde(String(args.destParent ?? ""));
        const { valid, name } = parseRepoSpec(spec);
        if (!valid || !name) throw new Error(`not a repository: ${spec}`);
        if (!this.isDir(destParent)) throw new Error(`no such directory: ${destParent}`);
        const dest = childPath(destParent, name);
        if (this.isDir(dest)) throw new Error(`a folder already exists at ${dest}`);
        const parent = normalize(destParent);
        this.state.filesystem[parent] = [
          ...(this.state.filesystem[parent] ?? []),
          { name, is_dir: true },
        ];
        return this.addProject(dest);
      }
      case "gh_status":
        return fx.ghStatus;
      case "gh_repo_list":
        return fx.ghRepos;
      default:
        throw new Error("unknown op");
    }
  }

  /** What a freshly connected host pushes: the held tool-use prompt (so the
   *  waiting agent shows its approval card without a turn having to run) and a
   *  live turn on the running agent. */
  private bootstrap() {
    this.event("agent:event", {
      agent_id: "pamukkale",
      event: {
        type: "control_request",
        request_id: fx.PENDING_REQUEST_ID,
        request: {
          subtype: "can_use_tool",
          tool_use_id: fx.PENDING_TOOL_USE_ID,
          tool_name: "Bash",
        },
      },
    });
    this.later(() => this.runTurn("arabia", "continue"), 1200);
  }
}

/** `SocketFactory` that plugs the mock host into the real client. The expected
 *  host key is ignored — there is no handshake to fail — but a key is still
 *  reported, so pinning behaves as it does against a real host. */
export function mockSocket(opts: MockOptions = {}): SocketFactory {
  return async (_url, handlers) => {
    const host = new MockHost((frame) => handlers.onMessage(JSON.stringify(frame)), opts);
    host.onClose = (code) => {
      host.close();
      handlers.onClose(code);
    };
    handlers.onOpen();
    const socket: Socket = {
      hostKey: MOCK_HOST_KEY,
      // There is no network under the mock host, so it is always the near path.
      via: "lan",
      send: (text) => host.receive(text),
      close: () => host.close(),
    };
    return socket;
  };
}
