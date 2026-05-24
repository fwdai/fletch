<script lang="ts">
  import WorkspaceBar from "./lib/WorkspaceBar.svelte";
  import AgentList from "./lib/AgentList.svelte";
  import AgentPanes from "./lib/AgentPanes.svelte";
  import SpawnDialog from "./lib/SpawnDialog.svelte";
  import { store } from "./lib/store.svelte";
  import { onMount } from "svelte";

  onMount(() => {
    store.init();
  });

  let spawnOpen = $state(false);
</script>

<div class="app">
  <WorkspaceBar />
  <div class="body">
    <aside class="sidebar">
      <div class="sidebar-header">
        <span class="sidebar-title">Agents</span>
        <button class="primary" onclick={() => (spawnOpen = true)} disabled={!store.workspace}>
          + Spawn
        </button>
      </div>
      <AgentList />
    </aside>
    <main class="main">
      <AgentPanes />
    </main>
  </div>
  {#if spawnOpen}
    <SpawnDialog onClose={() => (spawnOpen = false)} />
  {/if}
</div>

<style>
  .app { display: flex; flex-direction: column; height: 100%; }
  .body { display: flex; flex: 1; min-height: 0; }
  .sidebar {
    width: 260px;
    background: var(--bg-elevated);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
  }
  .sidebar-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px;
    border-bottom: 1px solid var(--border);
  }
  .sidebar-title {
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    font-size: 11px;
    letter-spacing: 0.5px;
  }
  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; }
</style>
