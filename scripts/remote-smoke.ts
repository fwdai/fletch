#!/usr/bin/env bun
/**
 * End-to-end smoke test for the paired-device remote server
 * (`src-tauri/src/remote/`, contract in `docs/remote-protocol.md`).
 *
 * Pairs (or says hello with a saved credential), prints the workspace summary,
 * then prints every forwarded event for 20 seconds.
 *
 * Usage
 *   # first run: mint a code in Settings › General › Mobile devices › Pair a device
 *   bun scripts/remote-smoke.ts ws://192.168.1.24:47285/ws --token K7PQ2M9X
 *
 *   # later runs: the device token from the first run is reused automatically
 *   bun scripts/remote-smoke.ts ws://192.168.1.24:47285/ws
 *
 *   # or pass one explicitly
 *   bun scripts/remote-smoke.ts ws://192.168.1.24:47285/ws --device-token <43-char secret>
 *
 * Flags
 *   --token <code>          8-character pairing code; pairs and saves the device token
 *   --device-token <secret> device token to `hello` with
 *   --seconds <n>           how long to listen for events (default 20)
 *   --store <path>          where the device token is cached
 *                           (default $TMPDIR/fletch-remote-smoke.json)
 *
 * Exits 0 on success, 1 on any protocol or connection failure.
 */

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

interface Args {
  url: string;
  token?: string;
  deviceToken?: string;
  seconds: number;
  store: string;
}

function parseArgs(argv: string[]): Args {
  const positional: string[] = [];
  const flags: Record<string, string> = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg.startsWith("--")) {
      const [name, inline] = arg.slice(2).split("=", 2);
      flags[name] = inline ?? argv[++i] ?? "";
    } else {
      positional.push(arg);
    }
  }
  const url = positional[0];
  if (!url) {
    console.error("usage: bun scripts/remote-smoke.ts ws://host:port/ws [--token CODE]");
    process.exit(1);
  }
  return {
    url,
    token: flags.token || undefined,
    deviceToken: flags["device-token"] || undefined,
    seconds: Number(flags.seconds ?? 20),
    store: flags.store || join(tmpdir(), "fletch-remote-smoke.json"),
  };
}

function readSavedToken(store: string, url: string): string | undefined {
  if (!existsSync(store)) return undefined;
  try {
    const saved = JSON.parse(readFileSync(store, "utf8")) as Record<string, string>;
    return saved[url];
  } catch {
    return undefined;
  }
}

function saveToken(store: string, url: string, token: string): void {
  let saved: Record<string, string> = {};
  if (existsSync(store)) {
    try {
      saved = JSON.parse(readFileSync(store, "utf8")) as Record<string, string>;
    } catch {
      saved = {};
    }
  }
  saved[url] = token;
  writeFileSync(store, JSON.stringify(saved, null, 2), { mode: 0o600 });
}

type Reply = { id: string; ok: boolean; result?: unknown; error?: string };
type Event = { event: string; payload: unknown };

/** Minimal request/response client over the raw WebSocket. */
class Client {
  private pending = new Map<string, (reply: Reply) => void>();
  private next = 0;
  onEvent: ((event: Event) => void) | null = null;

  constructor(private socket: WebSocket) {
    socket.addEventListener("message", (ev) => {
      const frame = JSON.parse(String(ev.data)) as Reply | Event;
      if ("event" in frame) {
        this.onEvent?.(frame);
        return;
      }
      const resolve = this.pending.get(frame.id);
      if (!resolve) {
        console.warn(`! reply for unknown id ${frame.id}`);
        return;
      }
      this.pending.delete(frame.id);
      resolve(frame);
    });
  }

  request(op: string, args: Record<string, unknown> = {}): Promise<Reply> {
    const id = `smoke-${++this.next}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${op} timed out`));
      }, 30_000);
      this.pending.set(id, (reply) => {
        clearTimeout(timer);
        resolve(reply);
      });
      this.socket.send(JSON.stringify({ id, op, args }));
    });
  }

  async call(op: string, args: Record<string, unknown> = {}): Promise<unknown> {
    const reply = await this.request(op, args);
    if (!reply.ok) throw new Error(`${op}: ${reply.error}`);
    return reply.result;
  }
}

function connect(url: string): Promise<WebSocket> {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    socket.addEventListener("open", () => resolve(socket));
    socket.addEventListener("error", () => reject(new Error(`cannot connect to ${url}`)));
    socket.addEventListener("close", (ev) => {
      // 4001/4003/4004 are the protocol's auth failures; anything else here is
      // a normal teardown after we're done.
      if (ev.code >= 4000) {
        console.error(`\n✗ host closed the connection: ${ev.code} ${ev.reason}`);
        process.exit(1);
      }
    });
  });
}

interface Workspace {
  projects?: { id: string; name?: string }[];
  agents?: { id: string; status?: string; provider?: string; archive?: unknown }[];
}

function printWorkspace(workspace: unknown): void {
  if (!workspace || typeof workspace !== "object") {
    console.log("  (no workspace — no project is pinned yet)");
    return;
  }
  const ws = workspace as Workspace;
  const live = (ws.agents ?? []).filter((a) => !a.archive);
  console.log(`  projects: ${ws.projects?.length ?? 0}`);
  console.log(`  agents:   ${live.length} live / ${ws.agents?.length ?? 0} total`);
  for (const agent of live.slice(0, 10)) {
    console.log(`    · ${agent.id.padEnd(16)} ${agent.provider ?? "?"} ${agent.status ?? "?"}`);
  }
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  console.log(`→ connecting to ${args.url}`);
  const socket = await connect(args.url);
  const client = new Client(socket);

  if (args.token) {
    console.log(`→ pair with code ${args.token}`);
    const result = (await client.call("pair", {
      token: args.token,
      device: { name: "remote-smoke", platform: "cli", appVersion: "0.0.0" },
    })) as { deviceId: string; deviceToken: string; host: Record<string, string> };
    console.log(`✓ paired as ${result.deviceId}`);
    console.log(`  host: ${result.host.name} (${result.host.os}, v${result.host.appVersion})`);
    saveToken(args.store, args.url, result.deviceToken);
    console.log(`  device token saved to ${args.store}`);
    // `pair` pushes no snapshot by contract — the get_workspace below is it.
  } else {
    const deviceToken = args.deviceToken ?? readSavedToken(args.store, args.url);
    if (!deviceToken) {
      console.error(`✗ no device token for ${args.url}; pass --token <pairing code> first`);
      process.exit(1);
    }
    console.log("→ hello");
    const result = (await client.call("hello", {
      deviceToken,
      client: { name: "remote-smoke", platform: "cli", appVersion: "0.0.0" },
    })) as { host: Record<string, string>; workspace: unknown };
    console.log(`✓ hello accepted`);
    console.log(`  host: ${result.host.name} (${result.host.os}, v${result.host.appVersion})`);
    console.log("\nworkspace (from the hello result):");
    printWorkspace(result.workspace);
  }

  console.log("\n→ get_workspace");
  printWorkspace(await client.call("get_workspace"));

  // The allowlist should hold: a denied op answers, it does not disconnect.
  const denied = await client.request("db_select", { table: "settings" });
  console.log(
    denied.ok
      ? "✗ db_select was answered — the denylist is broken"
      : `✓ db_select rejected: ${denied.error}`,
  );

  console.log(`\n→ listening for events for ${args.seconds}s (Ctrl-C to stop)\n`);
  let count = 0;
  client.onEvent = (event) => {
    count++;
    const payload = JSON.stringify(event.payload);
    console.log(
      `  ${new Date().toISOString().slice(11, 19)} ${event.event.padEnd(26)} ${
        payload.length > 160 ? `${payload.slice(0, 160)}…` : payload
      }`,
    );
  };
  await new Promise((resolve) => setTimeout(resolve, args.seconds * 1000));
  console.log(`\n✓ done — ${count} event(s) received`);
  socket.close(1000, "smoke test complete");
}

main().catch((e) => {
  console.error(`✗ ${e instanceof Error ? e.message : String(e)}`);
  process.exit(1);
});
