import { useEffect, useMemo, useState } from "react";
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

/** One matched menu entry: a provider slash command, or a library skill
 *  offered under its slugged invocation token. */
type Entry =
  | { kind: "command"; cmd: SlashCommand }
  | { kind: "skill"; command: string; description: string };

/** The "/" source: provider-specific slash commands (built-ins + commands
 *  discovered from disk), then — for new-agent composers — library skills.
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
  // Seed synchronously (built-ins + anything already cached) so the list is
  // never empty on mount, then refresh from disk. discoverCommands also
  // populates the module cache that the store's passthroughSlashName reads.
  const [commands, setCommands] = useState<SlashCommand[]>(() => commandsFor(provider, projectDir));
  useEffect(() => {
    let live = true;
    setCommands(commandsFor(provider, projectDir));
    discoverCommands(provider, projectDir).then((cmds) => {
      if (live) setCommands(cmds);
    });
    return () => {
      live = false;
    };
  }, [provider, projectDir]);

  const skills = useAppStore((s) => s.skills);
  // Skills whose slug collides with a provider command are dropped inside
  // invocableSkills — the same rule the send-time resolver applies.
  const skillEntries = useMemo(
    () => (includeSkills ? invocableSkills(skills, provider, projectDir) : []),
    [includeSkills, skills, provider, projectDir],
  );

  const matched = useMemo<Entry[]>(() => {
    if (query === null) return [];
    const q = query.toLowerCase();
    return [
      ...commands
        .filter((c) => c.name.toLowerCase().startsWith(q))
        .map((cmd): Entry => ({ kind: "command", cmd })),
      ...skillEntries
        .filter((s) => s.command.startsWith(q))
        .map(
          (s): Entry => ({ kind: "skill", command: s.command, description: s.skill.description }),
        ),
    ];
  }, [commands, skillEntries, query]);

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
