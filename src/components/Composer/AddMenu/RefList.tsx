import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { FlyoutOption } from "@/components/ui/FlyoutMenu";

/** One thing the list offers: an issue, a pull request, a skill. */
export interface RefEntry {
  id: string;
  /** Its short reference, set in mono ahead of the title: `#412`, `ENG-88`,
   *  `/review`. */
  tag: string;
  title: string;
  /** The heading it is listed under, where the list has several kinds. */
  group?: string;
}

/** A side panel's list of things to reference in the prompt, narrowed by
 *  typing: the filter takes the focus as the panel opens, and ↵ picks the
 *  first match. Entries keep the order they came in; groups show their
 *  heading only when the list has more than one. */
export function RefList({
  entries,
  placeholder,
  empty,
  onPick,
}: {
  /** Null while the list is still loading. */
  entries: RefEntry[] | null;
  /** The filter's placeholder: what there is to search. */
  placeholder: string;
  /** What to say when there is nothing to list. */
  empty: string;
  onPick: (id: string) => void;
}) {
  const [query, setQuery] = useState("");
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => {
    field.current?.focus();
  }, []);

  const matched = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!entries || !q) return entries ?? [];
    return entries.filter(
      (e) => e.tag.toLowerCase().includes(q) || e.title.toLowerCase().includes(q),
    );
  }, [entries, query]);
  const grouped = new Set(matched.map((e) => e.group)).size > 1;

  return (
    <div className="fly-list">
      <input
        ref={field}
        className="add-filter text-sm"
        placeholder={placeholder}
        aria-label={placeholder}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key !== "Enter" || !matched[0]) return;
          e.preventDefault();
          onPick(matched[0].id);
        }}
      />
      {entries === null ? (
        <div className="add-note text-sm">Loading…</div>
      ) : matched.length === 0 ? (
        <div className="add-note text-sm">{entries.length === 0 ? empty : "No matches"}</div>
      ) : (
        matched.map((e, i) => (
          <Fragment key={e.id}>
            {grouped && e.group !== matched[i - 1]?.group && (
              <div className="add-group text-xs">{e.group}</div>
            )}
            <FlyoutOption
              title={e.title}
              lead={<span className="add-tag text-xs">{e.tag}</span>}
              name={e.title}
              onClick={() => onPick(e.id)}
            />
          </Fragment>
        ))
      )}
    </div>
  );
}
