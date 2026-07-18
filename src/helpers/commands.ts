// Slash-command resolution: matching typed `/…` input against passthrough
// provider commands, invocable library skills, and the curated set of Claude
// TUI-only commands that don't work over stream-json.

import { builtinCommandsFor, commandsFor, discoverCommands } from "../data/slashCommands";
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
