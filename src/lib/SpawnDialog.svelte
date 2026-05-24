<script lang="ts">
  import { store } from "./store.svelte";

  let { onClose }: { onClose: () => void } = $props();

  let name = $state("");
  let branch = $state("");
  let task = $state("");

  function suggestBranch() {
    if (!branch && name) {
      branch = "agent/" + name.toLowerCase().replace(/[^a-z0-9-]+/g, "-").slice(0, 32);
    }
  }

  async function onSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (!name.trim() || !branch.trim() || !task.trim()) return;
    await store.spawn(name.trim(), branch.trim(), task.trim());
    if (!store.lastError) onClose();
  }
</script>

<div
  class="backdrop"
  onclick={onClose}
  onkeydown={(e) => e.key === "Escape" && onClose()}
  role="presentation"
></div>
<div class="modal" role="dialog" aria-label="Spawn agent">
  <form onsubmit={onSubmit}>
    <h2>Spawn agent</h2>
    <label>
      <span>Name</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        bind:value={name}
        onblur={suggestBranch}
        placeholder="refactor-auth"
        autofocus
      />
    </label>
    <label>
      <span>Branch</span>
      <input bind:value={branch} placeholder="agent/refactor-auth" />
    </label>
    <label>
      <span>Task</span>
      <textarea
        bind:value={task}
        placeholder="What should this agent do? Plain English instructions."
        rows="5"
      ></textarea>
    </label>
    <div class="actions">
      <button type="button" onclick={onClose}>Cancel</button>
      <button type="submit" class="primary" disabled={store.busy}>
        {store.busy ? "Spawning…" : "Spawn"}
      </button>
    </div>
  </form>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    z-index: 10;
  }
  .modal {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 480px;
    max-width: calc(100vw - 32px);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 20px;
    z-index: 11;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
  h2 { margin: 0 0 16px; font-size: 14px; font-weight: 600; }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 12px;
  }
  label span {
    font-size: 11px;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
</style>
