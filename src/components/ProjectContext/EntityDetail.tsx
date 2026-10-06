import { useState } from "react";
import { api, type ContextEntity, type ContextGraph, type Rel } from "@/api";
import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { AssertionRow } from "./AssertionRow";
import { ASSERTION_KIND_LABEL, entityName, groupAssertions, headsAbout } from "./format";

const RELS: Rel[] = ["part_of", "depends_on", "serves"];

/** One entity: its record, its relations (with unlink / add link), and the
 *  current assertions about it grouped by kind and domain. The entity actions
 *  — Edit (a revision), Archive, Merge into — live in the header. */
export function EntityDetail({
  entity,
  graph,
  projectId,
  onEdit,
  onArchived,
}: {
  entity: ContextEntity;
  graph: ContextGraph;
  projectId: string;
  onEdit: () => void;
  onArchived: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const [confirmArchive, setConfirmArchive] = useState(false);
  const [mergeInto, setMergeInto] = useState("");
  const [linkRel, setLinkRel] = useState<Rel>("part_of");
  const [linkTo, setLinkTo] = useState("");

  const others = graph.entities.filter((e) => e.status === "active" && e.id !== entity.id);
  const relations = graph.relations.filter((r) => r.from === entity.id || r.to === entity.id);
  const groups = groupAssertions(headsAbout(graph, entity.id));

  const run = (p: Promise<unknown>) => {
    setError(null);
    p.catch((e) => setError(String(e)));
  };

  const archive = () => {
    if (!confirmArchive) {
      setConfirmArchive(true);
      return;
    }
    run(api.contextArchiveEntity(projectId, entity.id).then(onArchived));
  };

  const merge = () => {
    if (!mergeInto) return;
    run(api.contextMergeEntities(projectId, entity.id, mergeInto).then(onArchived));
  };

  const link = (from: string, to: string, rel: Rel, add: boolean) =>
    run(api.contextLink(projectId, { from, to, rel, add }));

  return (
    <>
      <div className="pc-detail-h">
        <h3 className="text-base">{entity.name}</h3>
        <Badge>{entity.kind}</Badge>
        <span className="pc-meta text-xs">{entity.slug}</span>
        <div className="pc-actions">
          <Button variant="ghost" size="sm" onClick={onEdit}>
            Edit
          </Button>
          <Button variant="ghost" size="sm" danger onClick={archive}>
            {confirmArchive ? "Confirm archive" : "Archive"}
          </Button>
        </div>
      </div>

      {entity.summary && <p className="text-sm">{entity.summary}</p>}
      {entity.aliases.length > 0 && (
        <div className="pc-chips text-xs">
          <span className="pc-meta">aliases</span>
          {entity.aliases.map((a) => (
            <span key={a} className="pc-chip">
              {a}
            </span>
          ))}
        </div>
      )}
      {entity.paths.length > 0 && (
        <div className="pc-chips text-xs">
          <span className="pc-meta">paths</span>
          {entity.paths.map((p) => (
            <span key={p} className="pc-chip mono">
              {p}
            </span>
          ))}
        </div>
      )}

      {others.length > 0 && (
        <div className="pc-inline-form text-sm">
          <span className="pc-meta">Merge into</span>
          <select
            className="ps-input text-sm"
            value={mergeInto}
            onChange={(e) => setMergeInto(e.target.value)}
            aria-label="Merge into"
          >
            <option value="">Choose an entity…</option>
            {others.map((e) => (
              <option key={e.id} value={e.id}>
                {e.name}
              </option>
            ))}
          </select>
          <Button variant="outline" size="sm" disabled={!mergeInto} onClick={merge}>
            Merge
          </Button>
        </div>
      )}

      <div>
        <div className="pc-group-t text-xs">Relations</div>
        {relations.length === 0 && <p className="pc-meta text-sm">None yet.</p>}
        {relations.map((r) => (
          <div key={`${r.from}:${r.rel}:${r.to}`} className="pc-rel-row text-sm">
            {r.from === entity.id ? (
              <span>
                {r.rel.replace("_", " ")} → {entityName(graph, r.to)}
              </span>
            ) : (
              <span>
                {entityName(graph, r.from)} {r.rel.replace("_", " ")} → this
              </span>
            )}
            <Button variant="link" size="sm" onClick={() => link(r.from, r.to, r.rel, false)}>
              Unlink
            </Button>
          </div>
        ))}
        {others.length > 0 && (
          <div className="pc-inline-form text-sm">
            <select
              className="ps-input text-sm"
              value={linkRel}
              onChange={(e) => setLinkRel(e.target.value as Rel)}
              aria-label="Relation"
            >
              {RELS.map((r) => (
                <option key={r} value={r}>
                  {r.replace("_", " ")}
                </option>
              ))}
            </select>
            <select
              className="ps-input text-sm"
              value={linkTo}
              onChange={(e) => setLinkTo(e.target.value)}
              aria-label="Link to"
            >
              <option value="">Choose an entity…</option>
              {others.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.name}
                </option>
              ))}
            </select>
            <Button
              variant="outline"
              size="sm"
              disabled={!linkTo}
              onClick={() => link(entity.id, linkTo, linkRel, true)}
            >
              Add link
            </Button>
          </div>
        )}
      </div>

      {groups.length === 0 ? (
        <p className="pc-meta text-sm">Nothing has been decided about this entity yet.</p>
      ) : (
        groups.map((g) => (
          <div key={g.kind}>
            <div className="pc-group-t text-xs">{ASSERTION_KIND_LABEL[g.kind]}</div>
            {g.domains.map((d) => (
              <div key={d.domain} className="pc-list">
                <span className="pc-meta text-xs">{d.domain}</span>
                {d.assertions.map((a) => (
                  <AssertionRow key={a.id} assertion={a} graph={graph} projectId={projectId} />
                ))}
              </div>
            ))}
          </div>
        ))
      )}

      {error && <div className="pc-error text-sm">{error}</div>}
    </>
  );
}
