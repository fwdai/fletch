import { useState } from "react";
import { Icon } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { SetSeg } from "@/components/SettingsScreen/primitives";
import { Button } from "@/components/ui/Button";
import { Select } from "@/components/ui/Select";
import { PROVIDERS } from "@/data/providers";
import type { NewCustomAgent } from "@/storage/customAgents";
import { useAppStore } from "@/store";
import { AssignPicker } from "./AssignPicker";
import { Mono } from "./Mono";
import { CA_HUES, INJECTION_HINT, MCP_HINT, MCP_SUPPORT, mcpAttachable } from "./shared";

/** Mutable editor form state. `model`/`effort` use "" as the "provider default"
 *  sentinel so the <select> has a concrete value; converted to null on save. */
interface Form {
  name: string;
  description: string;
  color: number;
  base: string;
  model: string;
  effort: string;
  instructions: string;
  skillIds: string[];
  mcpServerIds: string[];
}

const EFFORTS = [
  { value: "", label: "Default" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Med" },
  { value: "high", label: "High" },
];

export function AgentEditor({
  initial,
  isNew,
  onCancel,
  onSave,
}: {
  initial: NewCustomAgent;
  isNew: boolean;
  onCancel: () => void;
  onSave: (values: NewCustomAgent) => void;
}) {
  const modelsByAgent = useAppStore((s) => s.modelsByAgent);
  const providerFlags = useAppStore((s) => s.providerFlags);
  const skills = useAppStore((s) => s.skills);
  const mcpServers = useAppStore((s) => s.mcpServers);

  const [form, setForm] = useState<Form>({
    name: initial.name,
    description: initial.description,
    color: initial.color,
    base: initial.base,
    model: initial.model ?? "",
    effort: initial.effort ?? "",
    instructions: initial.instructions,
    skillIds: initial.skillIds,
    mcpServerIds: initial.mcpServerIds,
  });

  const set = (patch: Partial<Form>) => setForm((f) => ({ ...f, ...patch }));

  // Only providers the user hasn't disabled can be a base. Built-in providers
  // are the building blocks; a custom agent instances one of them.
  const bases = PROVIDERS.filter((p) => providerFlags[p.id] !== false);
  const base = PROVIDERS.find((p) => p.id === form.base) ?? bases[0];
  const models = modelsByAgent[form.base] ?? [];

  // Keep the model valid when the base changes: drop a selection the new base
  // doesn't offer back to its default.
  const onBase = (next: string) => {
    const nextModels = modelsByAgent[next] ?? [];
    const keep = nextModels.some((m) => m.id === form.model);
    set({ base: next, model: keep ? form.model : "" });
  };

  const canSave = form.name.trim().length > 0;

  const mcpSupport = MCP_SUPPORT[form.base] ?? "none";

  /** A saved id must be one the selected base can deliver: switching base
   *  (e.g. Claude → Codex with an HTTP server attached) must not persist
   *  assignments the spawn path would silently drop. Ids without a library row
   *  pass through — the delete path already detaches those. */
  const deliverable = (id: string) => {
    const server = mcpServers.find((s) => s.id === id);
    return !server || mcpAttachable(mcpSupport, server.transport);
  };

  const submit = () => {
    if (!canSave) return;
    onSave({
      name: form.name.trim(),
      description: form.description.trim(),
      color: form.color,
      base: form.base,
      model: form.model || null,
      effort: form.effort || null,
      instructions: form.instructions,
      skillIds: form.skillIds,
      mcpServerIds: form.mcpServerIds.filter(deliverable),
    });
  };

  return (
    <div className="set-pane">
      <div className="ca-editor">
        <button className="ca-ed-back iflex-center text-sm" onClick={onCancel}>
          <Icon name="chevL" size={13} /> All custom agents
        </button>

        {/* identity */}
        <div className="ca-ed-head flex-center">
          <Mono name={form.name} hue={form.color} size={46} />
          <input
            className="ca-ed-name text-xl"
            placeholder="Name this agent…"
            value={form.name}
            autoFocus
            onChange={(e) => set({ name: e.target.value })}
          />
          <div className="ca-hues flex-center">
            {CA_HUES.map((h) => (
              <button
                key={h}
                className={`ca-hue ${h === form.color ? "active" : ""}`}
                style={{ ["--h" as string]: h }}
                aria-label={`Color ${h}`}
                onClick={() => set({ color: h })}
              />
            ))}
          </div>
        </div>

        {/* description */}
        <div className="set-field ca-field">
          <label className="set-field-label text-sm">
            Description
            <span className="ca-field-hint">A short role tagline, shown in the picker</span>
          </label>
          <input
            className="set-text text-base"
            placeholder="e.g. Plans before coding · writes PLAN.md"
            value={form.description}
            onChange={(e) => set({ description: e.target.value })}
          />
        </div>

        {/* base + model */}
        <div className="ca-grid2">
          <div className="set-field">
            <label className="set-field-label text-sm">
              Base agent <span className="ca-req">*</span>
            </label>
            <Select
              ariaLabel="Base agent"
              value={form.base}
              onChange={onBase}
              options={bases.map((p) => ({
                value: p.id,
                label: p.label,
                icon: <ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={16} />,
              }))}
            />
          </div>
          <div className="set-field">
            <label className="set-field-label text-sm">Model</label>
            <Select
              ariaLabel="Model"
              value={form.model}
              disabled={base?.fixedModel}
              onChange={(v) => set({ model: v })}
              options={[
                { value: "", label: "Default model" },
                ...models.map((m) => ({ value: m.id, label: m.name })),
              ]}
            />
          </div>
        </div>

        <div className="ca-inject flex-center text-sm">
          <Icon name="zap" size={13} />
          <span>
            Instructions injected via <b>{INJECTION_HINT[form.base] ?? "the agent's CLI"}</b>
          </span>
        </div>

        {/* reasoning budget */}
        <div className="set-field ca-field">
          <label className="set-field-label text-sm">
            Reasoning budget
            <span className="ca-field-hint">Default thinking depth when this agent runs</span>
          </label>
          <SetSeg value={form.effort} options={EFFORTS} onChange={(v) => set({ effort: v })} />
        </div>

        {/* instructions */}
        <div className="set-field ca-field">
          <label className="set-field-label text-sm">
            Instructions
            <span className="ca-field-hint">The standing system prompt for this agent</span>
          </label>
          <textarea
            className="set-text ca-textarea text-base"
            value={form.instructions}
            placeholder="Describe this agent's role, how it should work, and what it should hand off…"
            onChange={(e) => set({ instructions: e.target.value })}
          />
        </div>

        {/* skills */}
        <div className="set-field ca-field">
          <label className="set-field-label text-sm">
            Skills
            <span className="ca-field-hint">
              Instruction documents this agent loads on demand — works on every base
            </span>
          </label>
          <AssignPicker
            items={skills.map((s) => ({ id: s.id, name: s.name, detail: s.description }))}
            selected={form.skillIds}
            onChange={(skillIds) => set({ skillIds })}
            emptyHint="No skills yet — create them under Settings → Skills."
          />
        </div>

        {/* tools (MCP servers) */}
        <div className="set-field ca-field">
          <label className="set-field-label text-sm">
            Tools (MCP)
            <span className="ca-field-hint">{MCP_HINT[mcpSupport]}</span>
          </label>
          <AssignPicker
            items={mcpServers.map((s) => {
              const unattachable = !mcpAttachable(mcpSupport, s.transport);
              const target = s.transport === "http" ? s.url : s.command;
              return {
                id: s.id,
                name: s.name,
                detail:
                  unattachable && mcpSupport === "stdio"
                    ? `${target} — HTTP servers aren't supported by this base`
                    : target,
                disabled: unattachable,
              };
            })}
            selected={form.mcpServerIds}
            onChange={(mcpServerIds) => set({ mcpServerIds })}
            emptyHint="No MCP servers yet — add them under Settings → Tools."
          />
        </div>

        <div className="ca-ed-foot flex-center">
          <span className="ca-grow" />
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!canSave} onClick={submit}>
            <Icon name="check" size={13} /> {isNew ? "Create agent" : "Save changes"}
          </Button>
        </div>
      </div>
    </div>
  );
}
