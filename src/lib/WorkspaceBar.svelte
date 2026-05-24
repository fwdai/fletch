<script lang="ts">
  import { store } from "./store.svelte";
  import { open } from "@tauri-apps/plugin-dialog";

  function basename(p: string): string {
    const parts = p.split("/").filter(Boolean);
    return parts[parts.length - 1] ?? p;
  }

  async function pickRepo() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected !== "string") return;
    const baseImage = prompt(
      "Tart base image name (run `tart list` to see options):",
      store.workspace?.base_image ?? "base-dev",
    );
    if (!baseImage) return;
    await store.setRepo(selected, baseImage);
  }
</script>

<header class="bar">
  <div class="left">
    <span class="logo">algiers</span>
    {#if store.workspace}
      <span class="repo" title={store.workspace.repo_path}>
        {basename(store.workspace.repo_path)}
      </span>
      <span class="base">base: {store.workspace.base_image}</span>
    {:else}
      <span class="repo dim">No repo selected</span>
    {/if}
  </div>
  <div class="right">
    <button onclick={pickRepo}>
      {store.workspace ? "Switch repo…" : "Choose repo…"}
    </button>
  </div>
</header>

<style>
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-elevated);
    height: 44px;
    flex-shrink: 0;
  }
  .left { display: flex; align-items: center; gap: 14px; }
  .logo {
    font-weight: 700;
    letter-spacing: -0.3px;
    color: var(--accent);
  }
  .repo { font-weight: 500; }
  .repo.dim { color: var(--text-muted); font-style: italic; font-weight: 400; }
  .base {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-muted);
    padding: 2px 6px;
    background: var(--bg-input);
    border-radius: 4px;
  }
</style>
