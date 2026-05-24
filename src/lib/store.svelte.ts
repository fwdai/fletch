import {
  api,
  onAgentOutput,
  onAgentStatus,
  type AgentRecord,
  type Workspace,
} from "./api";

type OutputHandler = (bytes: Uint8Array) => void;

class AppStore {
  workspace = $state<Workspace | null>(null);
  selectedAgentId = $state<string | null>(null);
  busy = $state(false);
  lastError = $state<string | null>(null);

  /** Per-agent xterm sinks. Registered when an AgentPane mounts; the
   *  store forwards `agent:output` events to the matching handler so
   *  bytes are never dropped if a pane was already open. */
  private outputSinks = new Map<string, OutputHandler>();

  get agents(): AgentRecord[] {
    return this.workspace?.agents ?? [];
  }

  async init() {
    this.workspace = await api.getWorkspace();

    await onAgentOutput((e) => {
      const sink = this.outputSinks.get(e.agent_id);
      if (sink) sink(new Uint8Array(e.bytes));
    });

    await onAgentStatus((e) => {
      const a = this.findAgent(e.agent_id);
      if (a) {
        a.status = e.status;
        if (e.last_error) a.last_error = e.last_error;
      }
    });
  }

  registerOutputSink(agentId: string, handler: OutputHandler) {
    this.outputSinks.set(agentId, handler);
    return () => this.outputSinks.delete(agentId);
  }

  findAgent(id: string): AgentRecord | undefined {
    return this.workspace?.agents.find((a) => a.id === id);
  }

  selectAgent(id: string) {
    this.selectedAgentId = id;
  }

  async setRepo(path: string, baseImage: string) {
    this.busy = true;
    this.lastError = null;
    try {
      this.workspace = await api.setRepo(path, baseImage);
    } catch (e) {
      this.lastError = String(e);
    } finally {
      this.busy = false;
    }
  }

  async spawn(name: string, branch: string, task: string) {
    this.busy = true;
    this.lastError = null;
    try {
      const rec = await api.spawnAgent(name, branch, task);
      // Server has updated workspace.json — refresh our snapshot so the new
      // agent shows up. Could also use the returned record directly but a
      // single source of truth keeps things simple.
      this.workspace = await api.getWorkspace();
      this.selectedAgentId = rec.id;
    } catch (e) {
      this.lastError = String(e);
    } finally {
      this.busy = false;
    }
  }

  async stop(id: string) {
    try {
      await api.stopAgent(id);
    } catch (e) {
      this.lastError = String(e);
    }
  }

  async discard(id: string) {
    try {
      await api.discardWorktree(id);
      this.workspace = await api.getWorkspace();
      if (this.selectedAgentId === id) this.selectedAgentId = null;
    } catch (e) {
      this.lastError = String(e);
    }
  }
}

export const store = new AppStore();
