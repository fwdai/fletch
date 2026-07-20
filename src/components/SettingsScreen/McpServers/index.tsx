import { useState } from "react";
import { CustomizeSwitch } from "@/components/SettingsScreen/CustomizeSwitch";
import { LibraryList } from "@/components/SettingsScreen/LibraryList";
import type { McpServer, NewMcpServer } from "@/storage/mcpServers";
import { useAppStore } from "@/store";
import { ServerEditor } from "./ServerEditor";

// Tools (MCP servers) settings pane: a list ⇄ editor switch over the shared
// server registry. Custom agents attach servers by id; at spawn the selection
// is snapshotted onto the session and delivered to providers that support MCP
// (claude, codex). All mutations go through the store slice.

function blankServer(): NewMcpServer {
  return { name: "", transport: "stdio", command: "", env: "", url: "", headers: "" };
}

type EditTarget =
  | { mode: "new"; initial: NewMcpServer }
  | { mode: "edit"; id: string; initial: NewMcpServer };

export function McpServersPane() {
  const servers = useAppStore((s) => s.mcpServers);
  const customAgents = useAppStore((s) => s.customAgents);
  const createMcpServer = useAppStore((s) => s.createMcpServer);
  const updateMcpServer = useAppStore((s) => s.updateMcpServer);
  const deleteMcpServer = useAppStore((s) => s.deleteMcpServer);
  const setLastError = useAppStore((s) => s.setLastError);

  const [editing, setEditing] = useState<EditTarget | null>(null);

  const startNew = () => setEditing({ mode: "new", initial: blankServer() });
  const startEdit = (s: McpServer) => setEditing({ mode: "edit", id: s.id, initial: s });

  /** How many agents attach a server — shown in the list. */
  const usedBy = (id: string) => customAgents.filter((a) => a.mcpServerIds.includes(id)).length;

  const save = async (values: NewMcpServer) => {
    try {
      if (editing?.mode === "edit") {
        await updateMcpServer(editing.id, values);
      } else {
        await createMcpServer(values);
      }
      setEditing(null);
    } catch (e) {
      setLastError(`Failed to save MCP server: ${e}`);
    }
  };

  const remove = async (s: McpServer) => {
    try {
      await deleteMcpServer(s.id);
    } catch (e) {
      setLastError(`Failed to delete MCP server: ${e}`);
    }
  };

  if (editing) {
    return (
      <ServerEditor
        initial={editing.initial}
        isNew={editing.mode === "new"}
        onCancel={() => setEditing(null)}
        onSave={save}
      />
    );
  }

  return (
    <LibraryList
      eyebrow="Settings · Customize"
      eyebrowAside={<CustomizeSwitch />}
      title="Tools (MCP)"
      desc="MCP servers your custom agents can attach as tools. Supported by Claude Code and Codex bases; other bases run without them. Running sessions keep the configuration they spawned with."
      newLabel="New server"
      emptyLabel="Add your first MCP server"
      icon="zap"
      items={servers}
      row={(s) => {
        const count = usedBy(s.id);
        return {
          name: s.name,
          badge: `${s.transport} · ${count === 1 ? "1 agent" : `${count} agents`}`,
          desc: (s.transport === "http" ? s.url : s.command) || "Not configured.",
        };
      }}
      onNew={startNew}
      onEdit={startEdit}
      onDelete={remove}
    />
  );
}
