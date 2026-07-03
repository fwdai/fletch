// Onboarding exhibits — small, real-feeling fragments of the Fletch UI,
// framed inside each feature beat. Built from the same visual language as the
// app so the tour previews the actual product. Ported from the design
// prototype (onboarding/exhibits.jsx).

import { useEffect, useState } from "react";
import { Icon, LandmarkGlyph } from "@/components/Icon";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Badge } from "@/components/ui/Badge";
import { PROVIDER_DETAIL } from "@/data/providerDetail";
import type { ProviderId } from "@/data/providers";
import { PROVIDERS, providerChip } from "@/data/providers";

// ── tiny faux window chrome for an exhibit ──────────────────────────
function ExBar({ title }: { title: string }) {
  return (
    <div className="ex-bar">
      <div className="dots">
        <i />
        <i />
        <i />
      </div>
      <span className="title">{title}</span>
    </div>
  );
}

// ── Exhibit 1 · parallel agents in isolated worktrees ───────────────
// Renders the real sidebar agent row (the `.agent` markup + classes from
// app.css) against mock data, so the preview matches the product exactly.
interface ParallelAgent {
  name: string;
  provider: ProviderId;
  task: string;
  status: "running" | "idle" | "error";
  add: number;
  rem: number;
  age: string;
  /** Idle agent with results the user hasn't reviewed — the "needs you" cue. */
  unseen?: boolean;
  active?: boolean;
}

const PARALLEL_AGENTS: ParallelAgent[] = [
  {
    name: "dolomites",
    provider: "claude",
    task: "Stripe portal sync",
    status: "running",
    add: 187,
    rem: 64,
    age: "4m",
    active: true,
  },
  {
    name: "andes",
    provider: "codex",
    task: "Zero-copy decoder",
    status: "running",
    add: 624,
    rem: 312,
    age: "12m",
  },
  {
    name: "caspian",
    provider: "cursor",
    task: "Diff repaint jitter",
    status: "idle",
    add: 14,
    rem: 22,
    age: "1h",
    unseen: true,
  },
  {
    name: "sierra",
    provider: "opencode",
    task: "v3 migration guide",
    status: "running",
    add: 240,
    rem: 12,
    age: "26m",
  },
];

// Mirrors AgentRow's RealRow markup (static, no store/interactivity).
function ExhibitAgentRow({ a }: { a: ParallelAgent }) {
  const working = a.status === "running";
  const rail = working ? "run" : a.status === "error" ? "err" : "idle";
  return (
    <div className={`agent ${a.active ? "active" : ""}`}>
      <span className={`ag-rail ${rail}`} />
      <div className="agent-row flex-center">
        <span className={`ag-name ${working ? "shimmer" : ""}`}>{a.name}</span>
        <span className="ag-prov-chip">
          <ProviderIcon slug={a.provider} {...providerChip(a.provider)} size={14} />
        </span>
        <span className="ag-slot iflex-center">
          <span className="ag-meta">
            {working && <span className="ag-loader" aria-label="Working" />}
            {!working && a.unseen && (
              <span className="ag-unseen" aria-label="New results to review" />
            )}
            {a.status === "error" && <Badge variant="err">error</Badge>}
          </span>
          <span className="ag-actions">
            <span className="ag-act iflex-center">
              <Icon name={working ? "stop" : "archive"} size={11} />
            </span>
          </span>
        </span>
      </div>
      <div className="agent-sub flex-center">
        <span className="a-task">{a.task}</span>
        <span className="a-diff">
          <span className="add">+{a.add}</span> <span className="del">−{a.rem}</span>
        </span>
        <span className="a-time">{a.age}</span>
      </div>
    </div>
  );
}

export function ExhibitParallel() {
  return (
    <div className="ob-exhibit-wrap ob-reveal" style={{ "--d": ".25s" } as React.CSSProperties}>
      <div className="ob-exhibit">
        <ExBar title="fletch — worktrees" />
        <div className="ex-side">
          <div className="proj">
            <div className="proj-h flex-center open">
              <Icon name="chevR" size={10} className="chev" />
              <span className="pname">fletch-core</span>
              <span className="pcount">4</span>
            </div>
            <div className="agents">
              {PARALLEL_AGENTS.map((a) => (
                <ExhibitAgentRow key={a.name} a={a} />
              ))}
            </div>
          </div>
        </div>
        <div className="ob-exhibit-cap">
          <span className="lvdot" />3 running · 1 waiting · isolated branches
        </div>
      </div>
    </div>
  );
}

// ── Exhibit 2 · any agent, one roof ─────────────────────────────────
export function ExhibitProviders() {
  const list = PROVIDERS.slice(0, 6);
  const [lit, setLit] = useState(0);
  useEffect(() => {
    setLit(0);
    const timers = list.map((_, i) =>
      setTimeout(() => setLit((n) => Math.max(n, i + 1)), 380 + i * 230),
    );
    return () => timers.forEach(clearTimeout);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div className="ob-exhibit-wrap ob-reveal" style={{ "--d": ".25s" } as React.CSSProperties}>
      <div className="ob-exhibit">
        <ExBar title="fletch — providers" />
        <div className="ex-providers">
          {list.map((p, i) => (
            <div key={p.id} className={`ex-prov ${i < lit ? "on" : ""}`}>
              <ProviderIcon slug={p.id} short={p.short} hue={p.hue} size={30} />
              <span className="meta">
                <span className="pl">{p.label}</span>
                <span className="ps">{PROVIDER_DETAIL[p.id].models}</span>
              </span>
              <span className="chk">
                <Icon name="check" size={11} strokeWidth={2} />
              </span>
            </div>
          ))}
        </div>
        <div className="ob-exhibit-cap">
          <span className="lvdot" />
          connected · switch per task, no lock-in
        </div>
      </div>
    </div>
  );
}

// ── Exhibit 3 · the control room (home) ─────────────────────────────
// Every worktree at a glance — the editorial home screen, in miniature.
interface RoomRow {
  name: string;
  landmark: string;
  task: string;
  status: "running" | "waiting" | "error";
  add: number;
  rem: number;
  meta: string;
}

const ROOM_ROWS: RoomRow[] = [
  {
    name: "patagonia",
    landmark: "patagonia",
    task: "Stripe portal sync",
    status: "running",
    add: 187,
    rem: 64,
    meta: "writing",
  },
  {
    name: "andes",
    landmark: "andes",
    task: "Zero-copy decoder",
    status: "running",
    add: 624,
    rem: 312,
    meta: "writing",
  },
  {
    name: "caspian",
    landmark: "caspian",
    task: "Diff repaint jitter",
    status: "waiting",
    add: 14,
    rem: 22,
    meta: "needs you",
  },
  {
    name: "sierra",
    landmark: "sierra",
    task: "v3 migration guide",
    status: "waiting",
    add: 240,
    rem: 12,
    meta: "needs you",
  },
  {
    name: "hokkaido",
    landmark: "hokkaido",
    task: "List virtualization",
    status: "error",
    add: 96,
    rem: 41,
    meta: "error",
  },
];

export function ExhibitRoom() {
  return (
    <div className="ob-exhibit-wrap ob-reveal" style={{ "--d": ".25s" } as React.CSSProperties}>
      <div className="ob-exhibit">
        <ExBar title="fletch — home" />
        <div className="ex-room">
          <div className="ex-room-head">
            <div className="ex-room-mark">
              <span className="d" />
              FLETCH
            </div>
            <div className="ex-room-when">
              Thu, Jun 4<span className="sep">·</span>9:24 AM
            </div>
          </div>
          <div className="ex-room-title">
            <em>four</em> worktrees still in flight.
          </div>
          <div className="ex-room-rows">
            {ROOM_ROWS.map((r) => (
              <div key={r.name} className={`ex-rr ${r.status}`}>
                <span className="gly">
                  <LandmarkGlyph name={r.landmark} size={13} strokeWidth={1.2} />
                </span>
                <span className="rn">{r.name}</span>
                <span className="rt">{r.task}</span>
                <span className="rr-r">
                  <span className="diff">
                    <span className="ad">+{r.add}</span>
                    <span className="rm">−{r.rem}</span>
                  </span>
                  <span className={`st ${r.status}`}>{r.meta}</span>
                </span>
              </div>
            ))}
          </div>
        </div>
        <div className="ob-exhibit-cap">
          <span className="lvdot" />
          one room · every project, every agent
        </div>
      </div>
    </div>
  );
}

// ── Exhibit 4 · live code, nothing hidden (parked — BEAT_CODE) ──────
// Kept for when the live-diff feature ships; not currently in the sequence.
function escHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
function hlTs(raw: string): string {
  let s = escHtml(raw);
  s = s.replace(/(\/\/[^\n]*)/g, '<span class="tkc">$1</span>');
  s = s.replace(/('[^']*'|`[^`]*`)/g, '<span class="tks">$1</span>');
  s = s.replace(
    /\b(const|let|await|async|return|if|export|function|new|throw|import|from|type)\b/g,
    '<span class="tkk">$1</span>',
  );
  s = s.replace(/\b(\d+)\b/g, '<span class="tkn">$1</span>');
  s = s.replace(/\b([A-Z][A-Za-z]{2,})\b/g, '<span class="tkt">$1</span>');
  return s;
}

interface DiffLine {
  op?: "add" | "rem";
  n?: number | string;
  o?: number | string;
  t: string;
  writing?: boolean;
  pending?: boolean;
}

const CODE_LINES: DiffLine[] = [
  { o: 42, t: "async function checkout(user: User) {" },
  { op: "rem", o: 43, t: "  return createSession(user);" },
  { op: "add", n: 43, t: "  if (user.subscription?.active) {" },
  { op: "add", n: 44, t: "    return openBillingPortal(user);" },
  { op: "add", n: 45, t: "  }" },
  { op: "add", n: 46, t: "  return createSession(user);", writing: true },
  { o: 47, t: "}", pending: true },
];

export function ExhibitCode() {
  return (
    <div className="ob-exhibit-wrap ob-reveal" style={{ "--d": ".25s" } as React.CSSProperties}>
      <div className="ob-exhibit">
        <ExBar title="fletch — code" />
        <div className="ex-code">
          <div className="ex-code-tabs">
            <span className="ex-ctab active">
              checkout.ts
              <span className="live" />
            </span>
            <span className="ex-ctab">portal.ts</span>
            <span className="ex-ctab">checkout.test.ts</span>
          </div>
          <div className="ex-note">
            <Icon name="thinking" size={12} />
            <span>
              Branch on subscription state — route existing subscribers through the billing portal
              instead of a fresh checkout.
            </span>
          </div>
          <div className="ex-diff">
            <div className="ex-hunk">@@ -42,3 +42,6 @@ async function checkout</div>
            {CODE_LINES.map((l, i) => {
              const cls = l.op === "add" ? "add" : l.op === "rem" ? "rem" : "";
              const writing = l.writing ? "writing" : "";
              const pend = l.pending ? "pend" : "";
              const sg = l.op === "add" ? "+" : l.op === "rem" ? "−" : " ";
              return (
                <div key={i} className={`ex-dl ${cls} ${writing} ${pend}`}>
                  <span className="n">{l.n || l.o || ""}</span>
                  <span className="sg">{sg}</span>
                  <span className="tx">
                    <span dangerouslySetInnerHTML={{ __html: hlTs(l.t) }} />
                    {l.writing && <span className="cur" />}
                  </span>
                </div>
              );
            })}
          </div>
        </div>
        <div className="ob-exhibit-cap">
          <span className="lvdot" />
          live diff · +34 −8 · follow along
        </div>
      </div>
    </div>
  );
}
