import { useEffect, useMemo, useState } from "react";
import { useEngine } from "@/api/engine";
import type { AcPick, AcSource } from "@/components/Composer/autocomplete/types";
import {
  commandsFor,
  discoverCommands,
  type LocalCommandAction,
  type SlashCommand,
} from "@/data/slashCommands";
import { invocableSkills } from "@/helpers";
import { useAppStore } from "@/store";

interface Args {
  query: string | null;
  provider: string;
  /** The agent's project root, used to discover project-level commands
   *  (`<projectDir>/.claude/commands`). Omit before a project is chosen; only
   *  user-level commands are then offered. */
  projectDir?: string;
  /** Offer library skills as `/` invocations after the provider's commands.
   *  Only the new-agent composer sets this: an invoked skill is attached at
   *  spawn (materialized into the sandbox), which an existing session can't
   *  receive mid-flight. */
  includeSkills?: boolean;
  /** Fired when an app-handled (local) command is picked; its text is not
   *  sent to the agent. */
  onLocalCommand?: (action: LocalCommandAction) => void;
}

/** One thing `/` invokes: a provider slash command, or a library skill
 *  offered under its slugged invocation token. */
export type SlashEntry =
  | { kind: "command"; cmd: SlashCommand }
  | { kind: "skill"; command: string; description: string };

/** The token an entry is invoked by, without the slash. */
export function slashName(entry: SlashEntry): string {
  return entry.kind === "command" ? entry.cmd.name : entry.command;
}

/** Everything `/` offers for a provider, in menu order: its slash commands
 *  (built-ins + commands discovered from disk), then — where `includeSkills` —
 *  the library skills. Shared by the `/` autocomplete and the add menu's
 *  command list, so both offer exactly the same set. */
export function useSlashEntries({
  provider,
  projectDir,
  includeSkills,
}: Pick<Args, "provider" | "projectDir" | "includeSkills">): SlashEntry[] {
  const engine = useEngine();
  const { hostId } = engine;
  // Seed synchronously (built-ins + anything already cached) so the list is
  // never empty on mount, then refresh from disk. discoverCommands also
  // populates the module cache that the store's passthroughSlashName reads.
  const [commands, setCommands] = useState<SlashCommand[]>(() =>
    commandsFor(hostId, provider, projectDir),
  );
  useEffect(() => {
    let live = true;
    setCommands(commandsFor(hostId, provider, projectDir));
    discoverCommands(engine, provider, projectDir).then((cmds) => {
      if (live) setCommands(cmds);
    });
    return () => {
      live = false;
    };
  }, [engine, hostId, provider, projectDir]);

  const skills = useAppStore((s) => s.skills);
  // Precedence mirrors the send-time resolver exactly (see invocableSkills):
  // built-ins beat skills — a colliding skill drops inside invocableSkills —
  // and skills beat discovered commands, filtered out of the command rows
  // below. Static on both sides, so what the menu offers is what runs no
  // matter when discovery's async cache fill lands.
  const skillEntries = useMemo(
    () => (includeSkills ? invocableSkills(skills, provider) : []),
    [includeSkills, skills, provider],
  );

  return useMemo<SlashEntry[]>(() => {
    // Skill tokens never equal a built-in name (those skills were dropped), so
    // this filter can only ever hide discovered commands.
    const claimed = new Set(skillEntries.map((s) => s.command));
    return [
      ...commands
        .filter((c) => !claimed.has(c.name))
        .map((cmd): SlashEntry => ({ kind: "command", cmd })),
      ...skillEntries.map(
        (s): SlashEntry => ({
          kind: "skill",
          command: s.command,
          description: s.skill.description,
        }),
      ),
    ];
  }, [commands, skillEntries]);
}

/** The "/" source: every slash entry whose name starts with what was typed.
 *  Picking a passthrough command or a skill inserts `/<name> ` for the user to
 *  send; a local command fires its action instead. Only fires at the start of
 *  a line. */
export function useCommandSource({
  query,
  provider,
  projectDir,
  includeSkills,
  onLocalCommand,
}: Args): AcSource {
  const entries = useSlashEntries({ provider, projectDir, includeSkills });
  const matched = useMemo<SlashEntry[]>(() => {
    if (query === null) return [];
    const q = query.toLowerCase();
    return entries.filter((e) => slashName(e).toLowerCase().startsWith(q));
  }, [entries, query]);

  const rows: AcSource["rows"] = matched.map((m) =>
    m.kind === "command"
      ? {
          title: `/${m.cmd.name}${m.cmd.hint ? ` ${m.cmd.hint}` : ""}`,
          detail: m.cmd.description,
          icon: { glyph: "terminal" },
        }
      : {
          title: `/${m.command}`,
          detail: m.description || "Skill",
          icon: { glyph: "notebookPen" },
        },
  );

  const pick = (i: number): AcPick | null => {
    const m = matched[i];
    if (!m) return null;
    if (m.kind === "skill") return { replace: `/${m.command} ` };
    if (m.cmd.kind === "local") {
      onLocalCommand?.(m.cmd.action);
      return { replace: "" };
    }
    return { replace: `/${m.cmd.name} ` };
  };

  return { trigger: "/", heading: "Slash commands", lineStart: true, query, rows, pick };
}
