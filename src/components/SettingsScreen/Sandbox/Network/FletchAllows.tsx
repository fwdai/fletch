import type { EgressOverview } from "@/api";
import { EGRESS_CATEGORIES } from "@/util/useEgressOverview";
import { SetDisclosure, SetRow } from "../../primitives";

/** The hosts every VM may reach without anyone allowing them: the providers'
 *  APIs, the registries, the toolchains, source hosting, the guest's own
 *  packages. Fletch maintains the list with the app and nothing here edits it;
 *  a project's `.fletch/settings.json` can turn an entry off for that project.
 *  Below them, the telemetry hosts the agent CLIs reach for on their own and
 *  work without, refused quietly under every posture but Open. A security
 *  feature should show what can leave the VM, so the whole list is here,
 *  folded away. */
export function FletchAllows({ overview }: { overview: EgressOverview }) {
  return (
    <SetDisclosure label={`What Fletch allows automatically · ${overview.template.length} hosts`}>
      <SetRow
        title="Always allowed"
        sub="What agents need to work. Maintained with the app; a project's settings file can turn one off."
      />
      {EGRESS_CATEGORIES.map(({ key, label }) => {
        const entries = overview.template.filter((t) => t.category === key);
        if (entries.length === 0) return null;
        return (
          <SetRow
            key={key}
            compact
            title={label}
            sub={<span className="mono">{entries.map((t) => t.pattern).join(" · ")}</span>}
          />
        );
      })}
      <SetRow
        title="Telemetry, blocked quietly"
        sub="Agent CLIs call these on their own and work without them. Only the Open posture lets them through."
      />
      <SetRow
        compact
        title="Telemetry and error reporting"
        sub={<span className="mono">{overview.quiet.join(" · ")}</span>}
      />
    </SetDisclosure>
  );
}
