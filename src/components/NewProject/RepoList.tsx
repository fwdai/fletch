import { useEffect, useMemo, useState } from "react";
import { api, type GhRepoSummary } from "../../api";
import { Icon } from "../Icon";

interface Props {
  selected: string | null;
  onSelect: (nameWithOwner: string) => void;
}

/** Searchable list of the user's GitHub repos for the clone picker.
 *  Fetches once; filtering is client-side. */
export function RepoList({ selected, onSelect }: Props) {
  const [repos, setRepos] = useState<GhRepoSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");

  useEffect(() => {
    let cancelled = false;
    api
      .ghRepoList()
      .then((r) => {
        if (!cancelled) setRepos(r);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const filtered = useMemo(() => {
    if (!repos) return [];
    const q = query.trim().toLowerCase();
    if (!q) return repos;
    return repos.filter(
      (r) =>
        r.name_with_owner.toLowerCase().includes(q) ||
        (r.description?.toLowerCase().includes(q) ?? false),
    );
  }, [repos, query]);

  if (error) {
    return <div className="np-list-msg e text-base">Couldn’t load your repos: {error}</div>;
  }
  if (!repos) {
    return <div className="np-list-msg text-base">Loading your repositories…</div>;
  }

  return (
    <div className="np-list-wrap">
      <div className="np-search flex-center">
        <Icon name="search" size={13} />
        <input
          autoFocus
          placeholder="Search your repositories…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>
      <div className="np-list">
        {filtered.length === 0 && (
          <div className="np-list-msg text-base">
            {query.trim() ? `No repositories match “${query}”.` : "No repositories found."}
          </div>
        )}
        {filtered.map((r) => (
          <button
            key={r.name_with_owner}
            className={`np-repo ${selected === r.name_with_owner ? "active" : ""}`}
            onClick={() => onSelect(r.name_with_owner)}
          >
            <Icon name="github" size={13} />
            <div className="np-repo-text">
              <div className="np-repo-name flex-center text-base">
                {r.name_with_owner}
                {r.is_private && <span className="np-priv text-2xs">Private</span>}
              </div>
              {r.description && (
                <div className="np-repo-desc truncate text-sm">{r.description}</div>
              )}
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}
