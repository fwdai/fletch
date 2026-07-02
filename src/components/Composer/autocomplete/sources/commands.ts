import { useMemo } from "react";
import type { AcPick, AcSource } from "@/components/Composer/autocomplete/types";
import { filterCommands } from "@/data/slashCommands";

interface Args {
  query: string | null;
  provider: string;
  /** Fired when an app-handled (local) command is picked; its text is not
   *  sent to the agent. */
  onLocalCommand?: (action: string) => void;
}

/** The "/" source: provider-specific slash commands. Picking a passthrough
 *  command inserts `/<name> ` for the user to send; a local command fires its
 *  action instead. Only fires at the start of a line. */
export function useCommandSource({ query, provider, onLocalCommand }: Args): AcSource {
  const matched = useMemo(
    () => (query === null ? [] : filterCommands(provider, query)),
    [provider, query],
  );

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
