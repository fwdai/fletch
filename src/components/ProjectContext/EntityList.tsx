import { useState } from "react";
import type { ContextEntity } from "@/api";
import { TextInput } from "@/components/ui/TextInput";
import { groupEntitiesByKind, KIND_LABEL, matchesSearch } from "./format";

/** The active entities, grouped by kind, with a search over slug, name and
 *  aliases. Selecting one opens its detail beside the list. */
export function EntityList({
  entities,
  selectedId,
  onSelect,
}: {
  entities: ContextEntity[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const groups = groupEntitiesByKind(entities.filter((e) => matchesSearch(e, query)));

  return (
    <div className="pc-list">
      <TextInput
        placeholder="Search entities…"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        aria-label="Search entities"
      />
      {groups.length === 0 && <p className="pc-meta text-sm">No entities match.</p>}
      {groups.map((g) => (
        <div key={g.kind}>
          <div className="pc-group-t text-xs">{KIND_LABEL[g.kind]}</div>
          {g.entities.map((e) => (
            <button
              key={e.id}
              type="button"
              className={`pc-row text-sm ${e.id === selectedId ? "active" : ""}`}
              onClick={() => onSelect(e.id)}
            >
              <span>{e.name}</span>
              <span className="pc-row-slug text-xs">{e.slug}</span>
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}
