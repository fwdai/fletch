import { useState } from "react";
import type { ContextOverview } from "@/api";
import { Toggle } from "@/components/Settings/Toggle";
import { Button } from "@/components/ui/Button";
import { Loader } from "@/components/ui/Loader";
import { useGate } from "@/store/capabilities";
import { useProjectSettings } from "@/util/useProjectSettings";
import { AddDecisionForm } from "./AddDecisionForm";
import { AddEntityForm } from "./AddEntityForm";
import { EntityDetail } from "./EntityDetail";
import { EntityList } from "./EntityList";
import { ENABLED_KEY, EXTRACT_KEY, flagOn } from "./format";
import { Preview } from "./Preview";
import { ReviewQueue } from "./ReviewQueue";
import { useContextOverview } from "./useContextOverview";

type Area = "entities" | "review" | "preview";
type Editor = { kind: "entity"; entityId?: string } | { kind: "decision"; about: string[] } | null;

/** The project page's Context tab: what the host knows about the project and
 *  serves its agents, for the user to inspect and curate. Loads the overview,
 *  follows `context:changed`, and hands the rest to `ContextTab`. */
export function ProjectContext({ projectId }: { projectId: string }) {
  const { overview, error } = useContextOverview(projectId);
  const { settings, save } = useProjectSettings(projectId);
  const gate = useGate("projectSettings");

  if (error) {
    return <div className="ps-state text-sm">Couldn’t load the project context: {error}</div>;
  }
  if (!overview) {
    return (
      <div className="ps-state iflex-center text-sm">
        <Loader variant="inherit" /> Loading…
      </div>
    );
  }
  return (
    <ContextTab
      projectId={projectId}
      overview={overview}
      settings={settings}
      onSave={save}
      locked={!!gate || settings === null}
    />
  );
}

/** The tab itself, over an overview already in hand: the two toggles, then
 *  Entities / Review queue / Preview. Pure over its props, so it renders in a
 *  test without a host. */
export function ContextTab({
  projectId,
  overview,
  settings,
  onSave,
  locked,
}: {
  projectId: string;
  overview: ContextOverview;
  settings: Record<string, string> | null;
  onSave: (key: string, value: string | null) => void;
  locked: boolean;
}) {
  const [area, setArea] = useState<Area>("entities");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [editor, setEditor] = useState<Editor>(null);

  const { graph, proposals, stats } = overview;
  // The settings hook is the live, writable read; the overview's flags cover
  // the beat before it answers.
  const enabled = settings ? flagOn(settings[ENABLED_KEY]) : overview.enabled;
  const extract = settings ? flagOn(settings[EXTRACT_KEY]) : overview.extract;
  // The tab works over active entities only: archived and merged ones are
  // history, and the store refuses a record about them. A selection that an
  // archive elsewhere made stale falls back to nothing.
  const active = graph.entities.filter((e) => e.status === "active");
  const selected = active.find((e) => e.id === selectedId) ?? null;

  const select = (id: string) => {
    setSelectedId(id);
    setEditor(null);
  };

  return (
    <section className="ps-section">
      <header className="ps-section-h">
        <h2 className="ps-section-t text-lg">Project context</h2>
        <p className="ps-section-lead text-sm">
          What this project is made of and what has been decided about it, served to every agent so
          it doesn&rsquo;t re-explore the project to work that out. Agents and the background
          extractor propose; you rule. Changing a decision records a new one over the old, with your
          reasoning — nothing here is edited in place.
        </p>
      </header>

      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-ctx-enabled">
          Serve project context to agents
        </label>
        <Toggle
          value={enabled}
          onChange={(next) => onSave(ENABLED_KEY, next ? null : "false")}
          disabled={locked}
        />
      </div>
      <div className="ps-field ps-name-row">
        <label className="ps-label text-sm" htmlFor="ps-ctx-extract">
          Extract decisions in the background
        </label>
        <Toggle
          value={extract}
          onChange={(next) => onSave(EXTRACT_KEY, next ? null : "false")}
          disabled={locked || !enabled}
        />
      </div>
      <p className="pc-meta text-xs">
        {stats.entities} entities · {stats.heads} current assertions · {stats.provisional}{" "}
        provisional · {stats.contradictions} contradictions · {stats.reads} reads
      </p>

      <nav className="pc-nav">
        {(
          [
            ["entities", "Entities"],
            ["review", "Review queue"],
            ["preview", "Preview"],
          ] as [Area, string][]
        ).map(([id, label]) => (
          <button
            key={id}
            type="button"
            className={`ps-tab iflex-center text-sm ${area === id ? "active" : ""}`}
            onClick={() => setArea(id)}
          >
            {label}
            {id === "review" && proposals.length > 0 && (
              <span className="pc-count text-xs">{proposals.length}</span>
            )}
          </button>
        ))}
      </nav>

      {area === "entities" && (
        <>
          <div className="pc-actions" style={{ marginBottom: 10 }}>
            <Button variant="outline" size="sm" onClick={() => setEditor({ kind: "entity" })}>
              Add entity
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={active.length === 0}
              onClick={() => setEditor({ kind: "decision", about: selected ? [selected.id] : [] })}
            >
              Add decision
            </Button>
          </div>
          <div className="pc-split">
            <EntityList entities={active} selectedId={selectedId} onSelect={select} />
            <div className="pc-detail">
              {editor?.kind === "entity" ? (
                <AddEntityForm
                  projectId={projectId}
                  initial={graph.entities.find((e) => e.id === editor.entityId)}
                  onDone={() => setEditor(null)}
                />
              ) : editor?.kind === "decision" ? (
                <AddDecisionForm
                  projectId={projectId}
                  entities={active}
                  initialAbout={editor.about}
                  onDone={() => setEditor(null)}
                />
              ) : selected ? (
                <EntityDetail
                  entity={selected}
                  graph={graph}
                  projectId={projectId}
                  onEdit={() => setEditor({ kind: "entity", entityId: selected.id })}
                  onArchived={() => setSelectedId(null)}
                />
              ) : (
                <p className="pc-meta text-sm">
                  {active.length === 0
                    ? "Nothing recorded yet. Add an entity, or let agents and the extractor propose."
                    : "Select an entity to see what has been decided about it."}
                </p>
              )}
            </div>
          </div>
        </>
      )}

      {area === "review" && (
        <ReviewQueue proposals={proposals} graph={graph} projectId={projectId} />
      )}

      {area === "preview" && <Preview projectId={projectId} entities={active} />}
    </section>
  );
}
