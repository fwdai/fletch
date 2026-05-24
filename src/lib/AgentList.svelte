<script lang="ts">
  import { store } from "./store.svelte";

  function statusColor(s: string): string {
    switch (s) {
      case "running":
        return "var(--success)";
      case "spawning":
        return "var(--warning)";
      case "error":
        return "var(--danger)";
      case "idle":
        return "var(--text-muted)";
      default:
        return "var(--text-muted)";
    }
  }
</script>

<div class="list">
  {#if store.agents.length === 0}
    <div class="empty">No agents yet. Click + Spawn to start one.</div>
  {/if}
  {#each store.agents as agent (agent.id)}
    <button
      class="row"
      class:selected={store.selectedAgentId === agent.id}
      onclick={() => store.selectAgent(agent.id)}
    >
      <span class="dot" style:background={statusColor(agent.status)}></span>
      <div class="rowtext">
        <div class="name">{agent.name}</div>
        <div class="meta">
          <span>{agent.status}</span>
          <span class="dim">·</span>
          <span class="branch">{agent.branch}</span>
        </div>
      </div>
    </button>
  {/each}
</div>

<style>
  .list { flex: 1; overflow-y: auto; padding: 4px; }
  .empty {
    padding: 16px;
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1.5;
    text-align: center;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px 10px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 6px;
    text-align: left;
    margin-bottom: 2px;
  }
  .row:hover { background: var(--bg-input); }
  .row.selected {
    background: var(--bg-input);
    border-color: var(--border-strong);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .rowtext { display: flex; flex-direction: column; min-width: 0; }
  .name {
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    font-size: 11px;
    color: var(--text-muted);
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .branch { font-family: var(--font-mono); }
  .dim { color: var(--border-strong); }
</style>
