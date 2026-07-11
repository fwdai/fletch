import { useState } from "react";
import { Icon } from "@/components/Icon";
import { SetSeg } from "@/components/SettingsScreen/primitives";
import { Button } from "@/components/ui/Button";
import type { McpTransport, NewMcpServer } from "@/storage/mcpServers";

const TRANSPORTS: { value: McpTransport; label: string }[] = [
  { value: "stdio", label: "Command" },
  { value: "http", label: "HTTP" },
];

export function ServerEditor({
  initial,
  isNew,
  onCancel,
  onSave,
}: {
  initial: NewMcpServer;
  isNew: boolean;
  onCancel: () => void;
  onSave: (values: NewMcpServer) => void;
}) {
  const [form, setForm] = useState<NewMcpServer>({
    name: initial.name,
    transport: initial.transport,
    command: initial.command,
    env: initial.env,
    url: initial.url,
    headers: initial.headers,
  });

  const set = (patch: Partial<NewMcpServer>) => setForm((f) => ({ ...f, ...patch }));

  const canSave =
    form.name.trim().length > 0 &&
    (form.transport === "stdio" ? form.command.trim().length > 0 : form.url.trim().length > 0);

  const submit = () => {
    if (!canSave) return;
    onSave({
      name: form.name.trim(),
      transport: form.transport,
      command: form.command.trim(),
      env: form.env,
      url: form.url.trim(),
      headers: form.headers,
    });
  };

  return (
    <div className="set-pane">
      <div className="ca-editor">
        <button className="ca-ed-back iflex-center text-sm" onClick={onCancel}>
          <Icon name="chevL" size={13} /> All servers
        </button>

        <div className="ca-ed-head flex-center">
          <input
            className="ca-ed-name text-xl"
            placeholder="Name this server…"
            value={form.name}
            autoFocus
            onChange={(e) => set({ name: e.target.value })}
          />
        </div>

        <div className="set-field ca-field">
          <label className="set-field-label text-sm">Transport</label>
          <SetSeg
            value={form.transport}
            options={TRANSPORTS}
            onChange={(v) => set({ transport: v })}
          />
        </div>

        {form.transport === "stdio" ? (
          <>
            <div className="set-field ca-field">
              <label className="set-field-label text-sm">
                Command <span className="ca-req">*</span>
                <span className="ca-field-hint">
                  Full command line, split on spaces (quoting isn't supported)
                </span>
              </label>
              <input
                className="set-text text-base mono"
                placeholder="npx -y @modelcontextprotocol/server-github"
                value={form.command}
                onChange={(e) => set({ command: e.target.value })}
              />
            </div>
            <div className="set-field ca-field">
              <label className="set-field-label text-sm">
                Environment
                <span className="ca-field-hint">KEY=VALUE, one per line</span>
              </label>
              <textarea
                className="set-text ca-textarea text-base mono"
                value={form.env}
                placeholder={"GITHUB_TOKEN=ghp_…"}
                onChange={(e) => set({ env: e.target.value })}
              />
            </div>
          </>
        ) : (
          <>
            <div className="set-field ca-field">
              <label className="set-field-label text-sm">
                URL <span className="ca-req">*</span>
              </label>
              <input
                className="set-text text-base mono"
                placeholder="https://mcp.example.com/mcp"
                value={form.url}
                onChange={(e) => set({ url: e.target.value })}
              />
            </div>
            <div className="set-field ca-field">
              <label className="set-field-label text-sm">
                Headers
                <span className="ca-field-hint">Name: value, one per line</span>
              </label>
              <textarea
                className="set-text ca-textarea text-base mono"
                value={form.headers}
                placeholder={"Authorization: Bearer …"}
                onChange={(e) => set({ headers: e.target.value })}
              />
            </div>
          </>
        )}

        <div className="ca-ed-foot flex-center">
          <span className="ca-grow" />
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!canSave} onClick={submit}>
            <Icon name="check" size={13} /> {isNew ? "Add server" : "Save changes"}
          </Button>
        </div>
      </div>
    </div>
  );
}
