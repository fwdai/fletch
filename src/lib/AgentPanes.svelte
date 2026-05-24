<script lang="ts">
  import { store } from "./store.svelte";
  import AgentTerminal from "./AgentTerminal.svelte";
</script>

<div class="panes">
  {#if !store.workspace}
    <div class="placeholder">
      <h2>Pick a repo to get started</h2>
      <p>Choose a git repository in the top bar. Each agent you spawn will
      get its own worktree under <code>.worktrees/</code> and a fresh Tart VM
      cloned from your base image.</p>
    </div>
  {:else if store.agents.length === 0}
    <div class="placeholder">
      <h2>No agents yet</h2>
      <p>Click <strong>+ Spawn</strong> in the sidebar to launch one.</p>
    </div>
  {:else if !store.selectedAgentId}
    <div class="placeholder">
      <h2>Select an agent</h2>
      <p>Pick one from the sidebar to attach to its terminal.</p>
    </div>
  {:else}
    {#each store.agents as agent (agent.id)}
      {#if store.selectedAgentId === agent.id}
        <AgentTerminal {agent} />
      {/if}
    {/each}
  {/if}
  {#if store.lastError}
    <div class="error" role="alert">
      {store.lastError}
      <button class="close" onclick={() => (store.lastError = null)}>×</button>
    </div>
  {/if}
</div>

<style>
  .panes {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--bg);
    position: relative;
  }
  .placeholder {
    margin: auto;
    text-align: center;
    color: var(--text-muted);
    max-width: 420px;
    padding: 32px;
  }
  .placeholder h2 {
    font-size: 16px;
    margin: 0 0 8px;
    color: var(--text);
    font-weight: 600;
  }
  .placeholder p { font-size: 13px; line-height: 1.6; margin: 0; }
  .placeholder code {
    font-family: var(--font-mono);
    font-size: 12px;
    background: var(--bg-input);
    padding: 1px 5px;
    border-radius: 4px;
  }
  .error {
    position: absolute;
    bottom: 16px;
    left: 16px;
    right: 16px;
    background: rgba(227, 100, 100, 0.15);
    border: 1px solid var(--danger);
    color: var(--text);
    padding: 10px 14px;
    border-radius: 6px;
    font-family: var(--font-mono);
    font-size: 12px;
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }
  .close {
    background: transparent;
    border: 0;
    padding: 0 4px;
    font-size: 16px;
    color: var(--text);
    cursor: pointer;
  }
</style>
