import type { PrState } from "@desktop/api/types/pr";
import { Icon } from "@desktop/components/Icon";
import { PrPill } from "../../components/ui";
import { useStore } from "../../store";

const MAX_TICKS = 12;

export function PrCard({ agentId, pr }: { agentId: string; pr: PrState }) {
  const checks = useStore((s) => s.prChecks[agentId]);
  const total = checks ? Math.min(MAX_TICKS, checks.total) : 0;
  const summary = !checks
    ? "No checks reported"
    : checks.pending
      ? `${checks.pending} checks running`
      : checks.failed
        ? `${checks.failed} failing`
        : `${checks.passed} checks passed`;
  return (
    <div className="pr-card rise">
      <div className="m">
        <PrPill pr={pr} />
        <span className="mono">{pr.url.replace("https://github.com/", "")}</span>
      </div>
      <div className="t">{pr.title}</div>
      <div className="m">
        {checks && (
          <span className="checks">
            {Array.from({ length: total }).map((_, i) => (
              <i
                // biome-ignore lint/suspicious/noArrayIndexKey: ticks are positional
                key={i}
                className={
                  i < checks.failed ? "f" : i >= checks.passed + checks.failed ? "p" : undefined
                }
              />
            ))}
          </span>
        )}
        <span className="mono">{summary}</span>
        <span style={{ flex: 1 }} />
        <a href={pr.url} target="_blank" rel="noreferrer" style={{ fontSize: 13 }}>
          GitHub
          <Icon name="external" size={12} />
        </a>
      </div>
    </div>
  );
}
