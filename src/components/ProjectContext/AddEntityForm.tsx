import { useState } from "react";
import { api, type ContextEntity, type EntityKind } from "@/api";
import { Button } from "@/components/ui/Button";
import { TextArea, TextInput } from "@/components/ui/TextInput";
import { ENTITY_KINDS, slugify, splitList } from "./format";

/** Create an entity, or — with `initial` — record a revision of one. The slug
 *  follows the name until the user touches it. */
export function AddEntityForm({
  projectId,
  initial,
  onDone,
}: {
  projectId: string;
  initial?: ContextEntity;
  onDone: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [slug, setSlug] = useState(initial?.slug ?? "");
  const [slugTouched, setSlugTouched] = useState(!!initial);
  const [kind, setKind] = useState<EntityKind>(initial?.kind ?? "topic");
  const [summary, setSummary] = useState(initial?.summary ?? "");
  const [aliases, setAliases] = useState(initial?.aliases.join(", ") ?? "");
  const [paths, setPaths] = useState(initial?.paths.join(", ") ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ready = name.trim() && slug.trim() && !busy;

  const submit = () => {
    setBusy(true);
    setError(null);
    api
      .contextRecordEntity(projectId, {
        id: initial?.id,
        slug: slug.trim(),
        kind,
        name: name.trim(),
        summary: summary.trim(),
        aliases: splitList(aliases),
        paths: splitList(paths),
      })
      .then(onDone)
      .catch((e) => {
        setError(String(e));
        setBusy(false);
      });
  };

  return (
    <div className="pc-form">
      <h3 className="text-base">{initial ? `Edit ${initial.name}` : "New entity"}</h3>
      <div className="ps-field">
        <label className="ps-label text-sm" htmlFor="pc-ent-name">
          Name
        </label>
        <TextInput
          id="pc-ent-name"
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            if (!slugTouched) setSlug(slugify(e.target.value));
          }}
        />
      </div>
      <div className="pc-form-row">
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-ent-slug">
            Slug
          </label>
          <TextInput
            id="pc-ent-slug"
            mono
            value={slug}
            onChange={(e) => {
              setSlugTouched(true);
              setSlug(e.target.value);
            }}
          />
        </div>
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-ent-kind">
            Kind
          </label>
          <select
            id="pc-ent-kind"
            className="ps-input text-sm"
            value={kind}
            onChange={(e) => setKind(e.target.value as EntityKind)}
          >
            {ENTITY_KINDS.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </select>
        </div>
      </div>
      <div className="ps-field">
        <label className="ps-label text-sm" htmlFor="pc-ent-summary">
          Summary
        </label>
        <TextArea
          id="pc-ent-summary"
          value={summary}
          onChange={(e) => setSummary(e.target.value)}
        />
      </div>
      <div className="ps-field">
        <label className="ps-label text-sm" htmlFor="pc-ent-aliases">
          Aliases (comma-separated)
        </label>
        <TextInput
          id="pc-ent-aliases"
          value={aliases}
          onChange={(e) => setAliases(e.target.value)}
        />
      </div>
      <div className="ps-field">
        <label className="ps-label text-sm" htmlFor="pc-ent-paths">
          Paths (comma-separated, repo-relative)
        </label>
        <TextInput
          id="pc-ent-paths"
          mono
          value={paths}
          onChange={(e) => setPaths(e.target.value)}
        />
      </div>
      {error && <div className="pc-error text-sm">{error}</div>}
      <div className="pc-form-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button variant="primary" disabled={!ready} onClick={submit}>
          {initial ? "Record revision" : "Add entity"}
        </Button>
      </div>
    </div>
  );
}
