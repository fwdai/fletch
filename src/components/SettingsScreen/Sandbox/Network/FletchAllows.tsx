import { Fragment } from "react";
import type { EgressOverview } from "@/api";
import { Button } from "@/components/ui/Button";
import { EGRESS_CATEGORIES } from "@/util/useEgressOverview";
import { SetDisclosure, SetRow } from "../../primitives";

/** The hosts every VM may reach without anyone allowing them: the providers'
 *  APIs, the registries, the toolchains, source hosting, the guest's own
 *  packages. Fletch maintains the list with the app and nothing here edits it;
 *  a project's `.fletch/settings.json` can turn an entry off for that project.
 *  Below them, the telemetry hosts the agent CLIs reach for on their own and
 *  work without, refused quietly under every posture but Open. A security
 *  feature should show what can leave the VM, so the whole list is here,
 *  folded away.
 *
 *  With `host`, a project's view: what it starts from, Fletch's list with the
 *  user's global changes, one row per host with the switch that turns it off
 *  or back on for that project alone. */
export function FletchAllows({
  overview,
  host,
}: {
  overview: EgressOverview;
  host?: (pattern: string) => { on: boolean; flip?: () => void };
}) {
  const list: { pattern: string; category: string | null }[] = host
    ? overview.base
    : overview.template;
  const groups = EGRESS_CATEGORIES.map(({ key, label }) => ({
    label,
    patterns: list.filter((t) => t.category === key).map((t) => t.pattern),
  }));
  if (host) {
    groups.push({
      label: "Your global list",
      patterns: overview.base.filter((e) => e.source === "global").map((e) => e.pattern),
    });
  }

  return (
    <SetDisclosure
      label={`${host ? "From Fletch and your list" : "What Fletch allows automatically"} · ${list.length} hosts`}
    >
      <SetRow
        title="Always allowed"
        sub="What agents need to work. Maintained with the app; a project's settings file can turn one off."
      />
      {groups.map(({ label, patterns }) =>
        patterns.length === 0 ? null : host ? (
          <Fragment key={label}>
            <SetRow title={label} />
            {patterns.map((pattern) => {
              const { on, flip } = host(pattern);
              return (
                <SetRow
                  key={pattern}
                  compact
                  title={<span className="mono">{pattern}</span>}
                  sub={on ? undefined : "Off for this project"}
                >
                  {flip && (
                    <Button
                      variant="ghost"
                      tip={on ? "For this project only, in the chosen settings file" : undefined}
                      onClick={flip}
                    >
                      {on ? "Turn off" : "Turn on"}
                    </Button>
                  )}
                </SetRow>
              );
            })}
          </Fragment>
        ) : (
          <SetRow
            key={label}
            compact
            title={label}
            sub={<span className="mono">{patterns.join(" · ")}</span>}
          />
        ),
      )}
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
