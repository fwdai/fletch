import { useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import type { NewSkill } from "@/storage/skills";

export function SkillEditor({
  initial,
  isNew,
  onCancel,
  onSave,
}: {
  initial: NewSkill;
  isNew: boolean;
  onCancel: () => void;
  onSave: (values: NewSkill) => void;
}) {
  const [form, setForm] = useState<NewSkill>({
    name: initial.name,
    description: initial.description,
    body: initial.body,
  });

  const set = (patch: Partial<NewSkill>) => setForm((f) => ({ ...f, ...patch }));
  const canSave = form.name.trim().length > 0;

  const submit = () => {
    if (!canSave) return;
    onSave({
      name: form.name.trim(),
      description: form.description.trim(),
      body: form.body,
    });
  };

  return (
    <div className="set-pane">
      <div className="ca-editor">
        <button className="ca-ed-back iflex-center text-sm" onClick={onCancel}>
          <Icon name="chevL" size={13} /> All skills
        </button>

        <div className="ca-ed-head flex-center">
          <input
            className="ca-ed-name text-xl"
            placeholder="Name this skill…"
            value={form.name}
            autoFocus
            onChange={(e) => set({ name: e.target.value })}
          />
        </div>

        <div className="set-field ca-field">
          <label className="set-field-label text-sm">
            Description
            <span className="ca-field-hint">
              One line telling the agent when this skill applies — it's all the agent sees until it
              opens the document
            </span>
          </label>
          <input
            className="set-text text-base"
            placeholder="e.g. How we review pull requests"
            value={form.description}
            onChange={(e) => set({ description: e.target.value })}
          />
        </div>

        <div className="set-field ca-field">
          <label className="set-field-label text-sm">
            Document
            <span className="ca-field-hint">
              Markdown, given to the agent as a file it reads when the task matches
            </span>
          </label>
          <textarea
            className="set-text ca-textarea text-base"
            value={form.body}
            placeholder="Write the instructions, steps, or reference material for this skill…"
            onChange={(e) => set({ body: e.target.value })}
          />
        </div>

        <div className="ca-ed-foot flex-center">
          <span className="ca-grow" />
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!canSave} onClick={submit}>
            <Icon name="check" size={13} /> {isNew ? "Create skill" : "Save changes"}
          </Button>
        </div>
      </div>
    </div>
  );
}
