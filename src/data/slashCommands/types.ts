import type { ProviderId } from "../providers";

// Slash commands surfaced in the composer's `/` autocomplete.
//
// Two flavors:
//
//  - `passthrough` — forwarded verbatim to the agent. For Claude these are
//    "skills": built-ins plus custom commands discovered from
//    `.claude/commands/*.md`. Picking one inserts `/<name> ` into the input;
//    the user then sends.
//
//  - `local` — handled by Fletch itself; the text never reaches the agent.
//    Picking one fires the action identified by `action`. None are defined
//    yet, but the slot is here so adding (e.g.) a `/clear` that wipes the
//    transcript view is a one-liner later.

/** The app-side action a `local` command fires when picked. `cli:*` shell out
 *  to `claude <subcommand>` and render the output; `app:*` drive existing
 *  store/UI capabilities. The dispatcher (store/localCommands.ts) switches on
 *  this exhaustively, so a new command means a new arm here and there. */
export type LocalCommandAction =
  | "cli:doctor"
  | "cli:mcp"
  | "app:cost"
  | "app:config"
  | "app:clear"
  | "app:resume";

export type SlashCommand =
  | {
      kind: "passthrough";
      name: string;
      description: string;
      hint?: string;
    }
  | {
      kind: "local";
      name: string;
      description: string;
      hint?: string;
      action: LocalCommandAction;
    };

/** Per-provider slash-command behavior — the command analogue of a
 *  `ChatAdapter` (see src/adapters). One instance per provider, registered in
 *  `COMMAND_ADAPTERS`. Adding a provider's commands is: add an adapter object
 *  and its registry entry here, plus a `CommandDiscovery` on the backend. */
export interface CommandAdapter {
  readonly id: ProviderId;
  /** Always-available commands not backed by a file on disk (built-ins and,
   *  later, `local` in-app commands). Merged ahead of discovered commands and
   *  win on name clash, so a custom file can't shadow a built-in. */
  readonly builtins: SlashCommand[];
  /** Whether this provider discovers user/project commands from disk via the
   *  backend `discover_slash_commands`. When false, discovery is skipped (no
   *  IPC) and only `builtins` are offered. */
  readonly discoverable: boolean;
}
