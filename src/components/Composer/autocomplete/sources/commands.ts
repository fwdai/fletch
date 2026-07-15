import { useEffect, useMemo, useState } from "react";
import type { AcPick, AcSource } from "@/components/Composer/autocomplete/types";
import {
  commandsFor,
  discoverCommands,
  type LocalCommandAction,
  type SlashCommand,
} from "@/data/slashCommands";

interface Args {
  query: string | null;
  provider: string;
  /** The agent's project root, used to discover project-level commands
   *  (`<projectDir>/.claude/commands`). Omit before a project is chosen; only
   *  user-level commands are then offered. */
  projectDir?: string;
  /** Fired when an app-handled (local) command is picked; its text is not
   *  sent to the agent. */
  onLocalCommand?: (action: LocalCommandAction) => void;
}

/** The "/" source: provider-specific slash commands (built-ins + commands
 *  discovered from disk). Picking a passthrough command inserts `/<name> ` for
 *  the user to send; a local command fires its action instead. Only fires at
 *  the start of a line. */
export function useCommandSource({ query, provider, projectDir, onLocalCommand }: Args): AcSource {
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

  const matched = useMemo(() => {
    if (query === null) return [];
    const q = query.toLowerCase();
    return commands.filter((c) => c.name.toLowerCase().startsWith(q));
  }, [commands, query]);

  const rows: AcSource["rows"] = matched.map((c) => ({
    title: `/${c.name}${c.hint ? ` ${c.hint}` : ""}`,
    detail: c.description,
    icon: { glyph: "terminal" },
  }));

  const pick = (i: number): AcPick | null => {
    const cmd = matched[i];
    if (!cmd) return null;
    if (cmd.kind === "local") {
      onLocalCommand?.(cmd.action);
      return { replace: "" };
    }
    return { replace: `/${cmd.name} ` };
  };

  return { trigger: "/", heading: "Slash commands", lineStart: true, query, rows, pick };
}
