import { HEAT_LABEL, type MapDomain } from "../types";

/** What the PM agent knows about the codebase — the map it checks new ideas
 *  against before writing anything on the board. */
export function ProductMap({ map }: { map: MapDomain[] }) {
  return (
    <div className="rm-map">
      <p className="rm-map-lede text-sm">
        What the PM agent knows about this codebase. It checks new ideas against this before writing
        anything down.
      </p>
      {map.map((d) => (
        <div key={d.id} className={`rm-dom flex-center heat-${d.heat}`}>
          <span className="rm-dom-bar" />
          <div className="rm-dom-id">
            <div className="rm-dom-l text-sm">{d.label}</div>
            <div className="rm-dom-n mono text-xs">{d.note}</div>
          </div>
          <div className="rm-dom-m">
            <span className="rm-dom-files mono text-xs">{d.files} files</span>
            <span className={`rm-dom-items mono text-xs ${d.items ? "on" : ""}`}>
              {d.items} planned
            </span>
          </div>
          <span className="rm-dom-heat mono text-xs">{HEAT_LABEL[d.heat]}</span>
        </div>
      ))}
    </div>
  );
}
