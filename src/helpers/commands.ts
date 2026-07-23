// Slash-command resolution: matching typed `/…` input against passthrough
// provider commands, invocable library skills, and the curated set of Claude
// TUI-only commands that don't work over stream-json. Also the app-side
// expansion of bodied commands (codex prompts), whose CLI never resolves
// `/name` itself over the managed transport.

import {
  builtinCommandsFor,
  cachedCommandsAcrossProjects,
  commandsFor,
  discoverCommands,
  type SlashCommand,
} from "../data/slashCommands";
import { type Skill, type SkillSnapshot, skillSlug } from "../storage/skills";

/** If `text` is a `/<name>` matching a known passthrough command for the
 *  given provider, return its bare name; otherwise null. `projectDir` scopes
 *  discovered project-level commands (see commandsFor). The result is used
 *  both to swap the optimistic user_message for a slash_command notice and to
 *  set a busy label. */
export function passthroughSlashName(
  providerId: string | undefined,
  text: string,
  projectDir?: string,
): string | null {
  if (!providerId || !text.startsWith("/")) return null;
  const first = text.split(/\s/)[0].slice(1);
  const match = commandsFor(providerId, projectDir).find(
    (c) => c.kind === "passthrough" && c.name === first,
  );
  return match ? match.name : null;
}

/** Library skills invocable as composer `/` commands, each under its slugged
 *  token. Precedence is static so the menu and the send path always agree,
 *  regardless of when async command discovery lands: built-ins win over skills
 *  (a skill named "init" can't shadow `/init`), and skills win over commands
 *  discovered from disk — deferring to those would make the winner depend on
 *  cache timing, and would make the same token run different things on
 *  different providers. A later skill sharing an earlier one's slug also
 *  drops, so resolution picks exactly what the menu offered. */
export function invocableSkills(
  skills: Skill[],
  providerId: string,
): { command: string; skill: Skill }[] {
  const taken = new Set(builtinCommandsFor(providerId).map((c) => c.name));
  const out: { command: string; skill: Skill }[] = [];
  for (const skill of skills) {
    const command = skillSlug(skill.name);
    if (taken.has(command)) continue;
    taken.add(command);
    out.push({ command, skill });
  }
  return out;
}

/** If `text` starts with `/<command>` naming an invocable skill, resolve the
 *  invocation: `snapshot` is the by-value skill to add to the spawn payload
 *  (materialized + indexed like an assigned skill) and `prompt` replaces the
 *  typed text — an explicit follow-it-now instruction, with everything after
 *  the command carried as arguments. Null when the text isn't a skill
 *  invocation; provider commands and plain messages flow through untouched. */
export function resolveSkillInvocation(
  skills: Skill[],
  providerId: string,
  text: string,
): { snapshot: SkillSnapshot; prompt: string } | null {
  if (!text.startsWith("/")) return null;
  const command = text.split(/\s/)[0].slice(1);
  if (!command) return null;
  const match = invocableSkills(skills, providerId).find((s) => s.command === command);
  if (!match) return null;
  const args = text.slice(command.length + 1).trim();
  const { name, description, body } = match.skill;
  const prompt =
    `Use the "${name}" skill for this task: read its file (listed in the Skills index in your instructions) and follow its instructions now.` +
    (args ? `\n\nArguments: ${args}` : "");
  return { snapshot: { name, description, body }, prompt };
}

/** Strip one pair of surrounding double quotes, so `FILES="a b"` yields the
 *  bare value while embedded quotes elsewhere stay untouched. */
function unquote(s: string): string {
  return s.length >= 2 && s.startsWith('"') && s.endsWith('"') ? s.slice(1, -1) : s;
}

/** Substitute codex-style placeholders into a prompt body: `$1`…`$9`
 *  positional (from whitespace-separated args), `$ARGUMENTS` all positionals
 *  joined, `$NAME` from `NAME=value` args (uppercase key, value may be
 *  quoted), `$$` a literal `$`. A missing positional becomes empty; an
 *  unmatched `$NAME` stays literal (it may be intentional text like `$PATH`).
 *  When the body references no placeholder at all, non-empty args are
 *  appended on their own paragraph so they're never silently dropped. */
export function substitutePromptArgs(body: string, args: string): string {
  // Tokens are runs of non-space/non-quote chars and quoted spans, so
  // `FILES="a b.ts"` survives as one token.
  const tokens = args.match(/(?:[^\s"]+|"[^"]*")+/g) ?? [];
  const named = new Map<string, string>();
  const positional: string[] = [];
  for (const tok of tokens) {
    const m = /^([A-Z][A-Z0-9_]*)=([\s\S]*)$/.exec(tok);
    if (m) named.set(m[1], unquote(m[2]));
    else positional.push(unquote(tok));
  }
  // Placeholder detection ignores `$$` escapes, so a body using only literal
  // dollars still gets its args appended below.
  const consumesArgs = /\$([1-9]|[A-Z][A-Z0-9_]*)/.test(body.replace(/\$\$/g, ""));
  const expanded = body.replace(/\$(\$|[1-9]|[A-Z][A-Z0-9_]*)/g, (whole, key: string) => {
    if (key === "$") return "$";
    if (key === "ARGUMENTS") return positional.join(" ");
    if (/^[1-9]$/.test(key)) return positional[Number(key) - 1] ?? "";
    return named.get(key) ?? whole;
  });
  return consumesArgs || !args ? expanded : `${expanded}\n\n${args}`;
}

/** Separator between the typed invocation and the substituted body in an
 *  app-expanded send. The zero-width space makes the seam STRUCTURAL: the
 *  char isn't typeable, so a literal user message can never accidentally read
 *  as an expansion — which matters because the render fold must classify
 *  historical transcript text with no send-time metadata, where even
 *  byte-exact recomputation couldn't tell a lookalike apart. Costs one
 *  invisible model-visible char per command send. */
export const EXPANSION_SEPARATOR = "\n\n\u200B";

/** If `text` invokes a bodied command in `commands`, return the full text to
 *  send: the typed invocation stays first — the turn row, transcript
 *  matching, and the user-bubble fold all key off it (see MessageItem) —
 *  then EXPANSION_SEPARATOR, then the body with the arguments substituted.
 *  Null when the text isn't such an invocation; verbatim-passthrough commands
 *  and plain messages flow through untouched. Pure over `commands` so it's
 *  directly testable; `expandSlashCommand` binds it to the discovery cache. */
export function expandCommandText(commands: SlashCommand[], text: string): string | null {
  if (!text.startsWith("/")) return null;
  const name = text.split(/\s/)[0].slice(1);
  if (!name) return null;
  const match = commands.find(
    (c) => c.kind === "passthrough" && c.body !== undefined && c.name === name,
  );
  // Re-narrow for TS: `find`'s predicate doesn't carry into the result type.
  if (match?.kind !== "passthrough" || match.body === undefined) return null;
  const args = text.slice(name.length + 1).trim();
  return `${text}${EXPANSION_SEPARATOR}${substitutePromptArgs(match.body, args)}`;
}

/** `expandCommandText` against the provider's cached command set (builtins +
 *  discovered). Callers on the send path should `await discoverCommands`
 *  first so a send racing the composer's async cache fill still expands. */
export function expandSlashCommand(
  providerId: string | undefined,
  text: string,
  projectDir?: string,
): string | null {
  if (!providerId) return null;
  return expandCommandText(commandsFor(providerId, projectDir), text);
}

/** If a user message's text is an app-expanded command send — the typed
 *  invocation, EXPANSION_SEPARATOR, the substituted body — return the typed
 *  invocation so the bubble can fold to a quiet chip, mirroring the
 *  optimistic slash_command notice. The separator's zero-width space can't be
 *  typed, so an ordinary message that merely looks like an expansion (even
 *  one byte-equal to what expansion would produce) never folds. Requiring the
 *  name to be a known bodied command is defense in depth; its cost is
 *  rendering in full while the discovery cache is cold — graceful, showing
 *  more rather than hiding. Pure over `commands` for testability;
 *  `expandedCommandInvocation` binds it to the discovery cache. */
export function expandedCommandLine(commands: SlashCommand[], text: string): string | null {
  if (!text.startsWith("/")) return null;
  const sep = text.indexOf(EXPANSION_SEPARATOR);
  if (sep <= 0) return null;
  const invocation = text.slice(0, sep);
  const name = invocation.split(/\s/)[0].slice(1);
  if (!name) return null;
  const known = commands.some(
    (c) => c.kind === "passthrough" && c.body !== undefined && c.name === name,
  );
  return known ? invocation : null;
}

/** `expandedCommandLine` against every cached discovery for the provider —
 *  render sites (MessageItem) don't know their project dir. Before discovery
 *  has run it returns null and the message renders in full. */
export function expandedCommandInvocation(
  providerId: string | undefined,
  text: string,
): string | null {
  if (!providerId || !text.startsWith("/")) return null;
  return expandedCommandLine(cachedCommandsAcrossProjects(providerId), text);
}

/** Claude built-in control commands that only work in its interactive TUI and
 *  do NOT resolve over the managed (custom) view's stream-json transport. Sent
 *  as a plain user message they never execute: Claude emits a transient reply
 *  that isn't persisted to the on-disk transcript, so the turn reconciles away
 *  and the message flashes then vanishes (see onSessionRecordsAppended). Bare
 *  names, no leading slash.
 *
 *  Deliberately EXCLUDES commands Fletch already handles in-app (clear, cost,
 *  config, resume, mcp, doctor — see data/slashCommands/claude.ts) and the
 *  working passthrough skills (help, compact, init): those must keep flowing.
 *  Kept small and curated on purpose — we only intercept commands we know are
 *  unsupported, never arbitrary unknown `/x` (which could be a not-yet-discovered
 *  custom command or a literal message). */
const CLAUDE_TUI_ONLY_COMMANDS = new Set([
  "usage",
  "agents",
  "login",
  "logout",
  "vim",
  "terminal-setup",
  "status",
]);

/** If `text` is a `/<name>` that is a known Claude TUI-only control command
 *  unsupported over stream-json — and NOT a command we actually handle for this
 *  provider (local or passthrough, built-in or discovered) — return its bare
 *  name; otherwise null. Scoped to claude; other providers never match. The
 *  store uses this to block the send and surface a "use the Native view" notice
 *  instead of dispatching a doomed turn that flashes and disappears. */
export async function unsupportedManagedCommand(
  providerId: string | undefined,
  text: string,
  projectDir?: string,
): Promise<string | null> {
  if (providerId !== "claude" || !text.startsWith("/")) return null;
  const first = text.split(/\s/)[0].slice(1);
  if (!CLAUDE_TUI_ONLY_COMMANDS.has(first)) return null;
  // A command we actually handle (e.g. a project `.claude/commands/usage.md`
  // that happens to share a curated name) always wins — never block something
  // that works here. Await discovery rather than reading the possibly-cold
  // cache: otherwise a `/usage` sent before the composer has run discovery
  // would be wrongly blocked. Only reached for the rare curated-name case, so
  // the extra round-trip is cheap.
  const commands = await discoverCommands(providerId, projectDir);
  if (commands.some((c) => c.name === first)) return null;
  return first;
}
