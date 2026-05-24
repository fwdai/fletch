<script lang="ts">
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { onDestroy, onMount } from "svelte";
  import { api } from "./api";
  import { store } from "./store.svelte";
  import type { AgentRecord } from "./api";

  let { agent }: { agent: AgentRecord } = $props();

  let containerEl: HTMLDivElement;
  let term: Terminal | null = null;
  let fit: FitAddon | null = null;
  let unregisterSink: (() => void) | null = null;
  let resizeObserver: ResizeObserver | null = null;

  onMount(() => {
    term = new Terminal({
      fontFamily: "ui-monospace, 'SF Mono', Menlo, monospace",
      fontSize: 13,
      cursorBlink: true,
      theme: {
        background: "#0e0f12",
        foreground: "#e6e8eb",
        cursor: "#5b8def",
        selectionBackground: "#3a3f4a",
      },
      allowProposedApi: true,
      convertEol: false,
      scrollback: 5000,
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(containerEl);
    fit.fit();

    term.onData((data) => {
      api.writeToAgent(agent.id, data).catch((e) => (store.lastError = String(e)));
    });

    term.onResize(({ cols, rows }) => {
      api.resizeAgent(agent.id, cols, rows).catch(() => {
        /* harmless if VM is gone */
      });
    });

    unregisterSink = store.registerOutputSink(agent.id, (bytes) => {
      term?.write(bytes);
    });

    resizeObserver = new ResizeObserver(() => fit?.fit());
    resizeObserver.observe(containerEl);
  });

  onDestroy(() => {
    unregisterSink?.();
    resizeObserver?.disconnect();
    term?.dispose();
  });

  async function onStop() {
    if (!confirm(`Stop agent "${agent.name}"? The VM will be destroyed.`)) return;
    await store.stop(agent.id);
  }

  async function onDiscard() {
    if (!confirm(`Discard worktree for "${agent.name}"? Uncommitted work will be lost.`)) return;
    await store.discard(agent.id);
  }
</script>

<div class="wrap">
  <div class="header">
    <div class="left">
      <span class="name">{agent.name}</span>
      <span class="branch">{agent.branch}</span>
      <span class="status" data-status={agent.status}>{agent.status}</span>
    </div>
    <div class="right">
      {#if agent.status === "running" || agent.status === "spawning"}
        <button onclick={onStop}>Stop</button>
      {/if}
      {#if agent.status === "stopped" || agent.status === "error"}
        <button onclick={onDiscard}>Discard worktree</button>
      {/if}
    </div>
  </div>
  {#if agent.last_error}
    <div class="errbar">{agent.last_error}</div>
  {/if}
  <div class="term" bind:this={containerEl}></div>
</div>

<style>
  .wrap { display: flex; flex-direction: column; flex: 1; min-height: 0; }
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-elevated);
  }
  .left { display: flex; align-items: center; gap: 10px; }
  .name { font-weight: 600; }
  .branch {
    font-family: var(--font-mono);
    font-size: 11px;
    padding: 2px 6px;
    background: var(--bg-input);
    border-radius: 4px;
  }
  .status {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    padding: 2px 6px;
    border-radius: 4px;
  }
  .status[data-status="running"] { color: var(--success); }
  .status[data-status="spawning"] { color: var(--warning); }
  .status[data-status="error"] { color: var(--danger); }
  .status[data-status="stopped"], .status[data-status="idle"] { color: var(--text-muted); }
  .errbar {
    background: rgba(227, 100, 100, 0.15);
    border-bottom: 1px solid var(--danger);
    color: var(--text);
    padding: 6px 12px;
    font-family: var(--font-mono);
    font-size: 11px;
  }
  .term {
    flex: 1;
    min-height: 0;
    padding: 8px;
    background: var(--bg);
    overflow: hidden;
  }
</style>
