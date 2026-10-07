import { useState } from "react";
import { api, type ContextEntity } from "@/api";
import { Button } from "@/components/ui/Button";
import { TextInput } from "@/components/ui/TextInput";
import { matchesSearch } from "./format";

/** "What would an agent see?" — a free-text query plus optional entity
 *  picks, compiled and rendered host-side exactly as for an agent; or the
 *  overview every agent on the project is spawned with. */
export function Preview({ projectId, entities }: { projectId: string; entities: ContextEntity[] }) {
  const [query, setQuery] = useState("");
  const [picks, setPicks] = useState<string[]>([]);
  const [filter, setFilter] = useState("");
  const [history, setHistory] = useState(false);
  const [output, setOutput] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const togglePick = (slug: string) =>
    setPicks((prev) => (prev.includes(slug) ? prev.filter((s) => s !== slug) : [...prev, slug]));

  const run = (overview: boolean) => {
    setBusy(true);
    setError(null);
    api
      .contextPreview(projectId, {
        entities: picks,
        query: query.trim() || undefined,
        paths: [],
        include_history: history,
        budget_chars: 0,
        overview,
      })
      .then(setOutput)
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  return (
    <div className="pc-form">
      <div className="ps-field">
        <label className="ps-label text-sm" htmlFor="pc-preview-query">
          Query — what the agent is about to work on
        </label>
        <TextInput
          id="pc-preview-query"
          value={query}
          placeholder="e.g. the roadmap queue, or a path"
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>
      {entities.length > 0 && (
        <div className="ps-field">
          <span className="ps-label text-sm">Entities to start from (optional)</span>
          <TextInput
            placeholder="Filter entities…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            aria-label="Filter entities"
          />
          <div className="pc-picks text-sm">
            {entities
              .filter((e) => matchesSearch(e, filter))
              .map((e) => (
                <label key={e.id}>
                  <input
                    type="checkbox"
                    checked={picks.includes(e.slug)}
                    onChange={() => togglePick(e.slug)}
                  />
                  {e.name} <span className="pc-meta text-xs">{e.slug}</span>
                </label>
              ))}
          </div>
        </div>
      )}
      <label className="text-sm">
        <input type="checkbox" checked={history} onChange={(e) => setHistory(e.target.checked)} />{" "}
        Include history
      </label>
      <div className="pc-form-actions">
        <Button variant="primary" disabled={busy} onClick={() => run(false)}>
          Preview
        </Button>
        <Button variant="outline" disabled={busy} onClick={() => run(true)}>
          Spawn overview
        </Button>
      </div>
      {error && <div className="pc-error text-sm">{error}</div>}
      {output !== null && <pre className="pc-preview text-sm">{output || "(empty)"}</pre>}
    </div>
  );
}
