/* ============================================================================
   Quorum — Product Showcase
   A single keyframe timeline drives a faithful mock of the Quorum app.
   render(time) rebuilds the whole visual state from scratch each frame, so the
   film is fully scrubbable and every beat is a tunable keyframe.
   ============================================================================ */
(function () {
  "use strict";
  const W = 1280, H = 800, DUR = 42000;

  /* ---------- easing ---------- */
  function cubicBezier(x1, y1, x2, y2) {
    const A = (a, b) => 1 - 3 * b + 3 * a, B = (a, b) => 3 * b - 6 * a, C = a => 3 * a;
    const calc = (t, a, b) => ((A(a, b) * t + B(a, b)) * t + C(a)) * t;
    const slope = (t, a, b) => 3 * A(a, b) * t * t + 2 * B(a, b) * t + C(a);
    function tForX(x) { let t = x; for (let i = 0; i < 5; i++) { const s = slope(t, x1, x2); if (s === 0) break; t -= (calc(t, x1, x2) - x) / s; } return t; }
    return p => p <= 0 ? 0 : p >= 1 ? 1 : calc(tForX(p), y1, y2);
  }
  const E = {
    settle: cubicBezier(.16, 1, .3, 1),   // apple expo-out
    inout:  cubicBezier(.65, 0, .35, 1),
    soft:   cubicBezier(.4, 0, .2, 1),
    out:    cubicBezier(.22, 1, .36, 1),
    linear: p => p,
    back:   p => { const c = 1.70158, c3 = c + 1; return 1 + c3 * Math.pow(p - 1, 3) + c * Math.pow(p - 1, 2); },
  };
  const lerp = (a, b, p) => a + (b - a) * p;
  const clamp01 = v => v < 0 ? 0 : v > 1 ? 1 : v;

  /* ---------- content ---------- */
  const PROMPT = "Add a command palette with fuzzy search across files, agents, and settings — ⌘K to open.";
  const REASON = "Global shortcuts are registered in App.tsx. I'll add a CommandPalette component, wire a ⌘K hotkey, and rank files, agents, and settings in one fuzzy list…";

  const PROVIDERS = [
    { slug: "claude",      hue: 28,  name: "Claude" },
    { slug: "codex",       hue: 145, name: "Codex" },
    { slug: "cursor",      hue: 215, name: "Cursor" },
    { slug: "antigravity", hue: 260, name: "Antigravity" },
    { slug: "opencode",    hue: 195, name: "OpenCode" },
    { slug: "pi",          hue: 320, name: "Pi" },
  ];

  // Agent names are landmark codenames (src/data/landmarks.ts); the task is the prompt.
  // A natural mix: 3 Claude, 2 Codex, 2 Cursor — not grouped, with irregular spawn
  // times (`spawn`), independent loader clocks (`sync`), and varied live states.
  const AGENTS = [
    { name: "dolomites", slug: "claude", hue: 28,  task: "Add a command palette with fuzzy search", add: 128, del: 12, age: "2m",  hero: true, spawn: 10150, sync: 2.4 },
    { name: "caspian",   slug: "codex",  hue: 145, task: "Refactor the diff streaming pipeline",     add: 64,  del: 9,  age: "4m",  spawn: 11550, sync: 2.75, idleAt: 29500 },
    { name: "yosemite",  slug: "cursor", hue: 215, task: "Audit OKLCH tokens for contrast",          add: 31,  del: 22, age: "6m",  spawn: 11980, sync: 2.05, idleAt: 24200 },
    { name: "patagonia", slug: "claude", hue: 28,  task: "Rework the sandbox-exec spawn path",        add: 22,  del: 14, age: "3m",  spawn: 12180, sync: 3.05, waitAt: 17400 },
    { name: "hokkaido",  slug: "codex",  hue: 145, task: "Fix the flaky git-status test",            add: 9,   del: 3,  age: "24m", spawn: 12380, sync: 2.5,  merged: "142" },
    { name: "andes",     slug: "cursor", hue: 215, task: "Wire the run panel to the task runner",     add: 47,  del: 1,  age: "8m",  spawn: 12950, sync: 2.9,  pr: "open", idleAt: 31500 },
    { name: "sierra",    slug: "claude", hue: 28,  task: "Polish the settings sheet",                 add: 18,  del: 6,  age: "1m",  spawn: 13520, sync: 1.95, idleAt: 33400 },
  ];
  // Thinking-effort levels for Claude on the new-agent screen (src/data/providerDetail.ts).
  const EFFORT = ["Low", "Med", "High", "xHigh"];

  const TOOLS = [
    { ic: "file", name: "Read",  arg: "src/App.tsx",                       res: "210 lines" },
    { ic: "grep", name: "Grep",  arg: '"useHotkeys" · src/**',             res: "6 matches" },
    { ic: "edit", name: "Edit",  arg: "components/CommandPalette/index.tsx", res: "+96 −4" },
    { ic: "term", name: "Bash",  arg: "bun test command-palette",          res: "✓ 14 passed" },
  ];

  const ICONS = {
    file: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/></svg>',
    grep: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>',
    edit: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>',
    term: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M4 17l6-6-6-6M12 19h8"/></svg>',
    check:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4"><path d="M20 6L9 17l-5-5"/></svg>',
    merge:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="18" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><path d="M6 21V9a9 9 0 0 0 9 9"/></svg>',
    push: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 19V6M5 12l7-7 7 7"/></svg>',
    commit:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="4"/><path d="M2 12h6M16 12h6"/></svg>',
    sparkle:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .962 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.962 0z"/><path d="M20 3v4M22 5h-4M4 17v2M5 18H3"/></svg>',
    stop:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="4" y="4" width="16" height="16" rx="2"/></svg>',
    pr:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="18" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><path d="M13 6h3a2 2 0 0 1 2 2v7"/><line x1="6" y1="9" x2="6" y2="21"/></svg>',
    archive:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="3" width="20" height="5" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/><path d="M10 12h4"/></svg>',
    help:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3"/><path d="M12 17h.01"/></svg>',
  };

  // One diff per file — the Code panel shows the file the agent is currently
  // editing, switching content (not just the tab) as it moves across files.
  const FILES = [
    { hunk: "@@ src/components/CommandPalette/index.tsx @@", lines: [
      { op: "ctx", t: 'import { useStore } from "../../store";' },
      { op: "add", t: 'import { useHotkeys } from "../../lib/hotkeys";' },
      { op: "add", t: 'import { useFuzzy } from "./fuzzy";' },
      { op: "ctx", t: '' },
      { op: "add", t: 'export function CommandPalette() {' },
      { op: "add", t: '  const open = useStore((s) => s.paletteOpen);' },
      { op: "add", t: '  useHotkeys("mod+k", () => useStore.toggle());' },
      { op: "add", t: '  const items = useFuzzy(q, useCommands());' },
      { op: "rem", t: '  return null;' },
      { op: "add", t: '  return <Palette items={items} open={open} />;' },
    ] },
    { hunk: "@@ src/store.ts @@", lines: [
      { op: "ctx", t: 'export const useStore = create((set) => ({' },
      { op: "ctx", t: '  agents: [],' },
      { op: "add", t: '  paletteOpen: false,' },
      { op: "add", t: '  setPaletteOpen: (v) => set({ paletteOpen: v }),' },
      { op: "add", t: '  toggle: () =>' },
      { op: "add", t: '    set((s) => ({ paletteOpen: !s.paletteOpen })),' },
    ] },
    { hunk: "@@ src/app.css @@", lines: [
      { op: "ctx", t: '/* command palette */' },
      { op: "add", t: '.command-palette {' },
      { op: "add", t: '  position: fixed; inset: 0; display: grid;' },
      { op: "add", t: '  place-items: start center; padding-top: 12vh;' },
      { op: "rem", t: '  z-index: 10;' },
      { op: "add", t: '  z-index: 200; backdrop-filter: blur(6px);' },
      { op: "add", t: '}' },
    ] },
  ];

  /* ---------- state ---------- */
  function base() {
    return {
      cam: { s: 1, fx: W / 2, fy: H / 2 },
      appOp: 0, appScale: .965,
      brand: { op: 0, ty: 20, bl: 14 }, brandTag: { op: 0, ty: 12 },
      newOp: 0, workOp: 0,
      typed: 0, prReveal: 0, prAgent: [0, 0, 0, 0, 0, 0],
      effortPress: 0, sendPress: 0,
      agentPop: [0, 0, 0, 0, 0, 0, 0], heroTask: 0,
      heroBadge: 0, heroStats: 0, heroCollapse: 0,
      thinkOp: 0, reason: 0,
      tIco: [0, 0, 0, 0], tName: [0, 0, 0, 0], tArg: [0, 0, 0, 0], tRes: [0, 0, 0, 0],
      resp: [0, 0, 0], chatScroll: 0,
      codeOp: 1, gitOp: 0, codeTab: [0, 0, 0], fileStream: [0, 0, 0],
      gitFile: [0, 0, 0], prCard: 0,
      o: [{ op: 0, ty: 26, bl: 8 }, { op: 0, ty: 26, bl: 8 }, { op: 0, ty: 26, bl: 8 }, { op: 0, ty: 26, bl: 8 }, { op: 0, ty: 26, bl: 8 }, { op: 0, ty: 26, bl: 8 }],
      close: { op: 0, ty: 28, bl: 10 }, scrim: 0,
      cur: { x: 560, y: 360, op: 0 }, ring: 0,
    };
  }
  let S = base();

  /* ---------- timeline ---------- */
  const T = [];
  const tw = (t, d, e, fn) => T.push({ t, d, e, fn });

  // ── Cold open (0–2.9s) ──
  tw(200, 1200, E.settle, p => { S.brand.op = p; S.brand.ty = lerp(20, 0, p); S.brand.bl = lerp(14, 0, p); });
  tw(700, 1000, E.settle, p => { S.brandTag.op = p; S.brandTag.ty = lerp(12, 0, p); });
  tw(2350, 650, E.soft, p => { S.brand.op = 1 - p; S.brand.ty = lerp(0, -16, p); S.brand.bl = lerp(0, 8, p); S.brandTag.op = 1 - p; S.brandTag.ty = lerp(0, -12, p); });

  // ── App entrance on the New-Agent screen (2.6–3.8s) ──
  tw(2600, 1400, E.settle, p => { S.appOp = p; S.appScale = lerp(.965, 1, p); S.newOp = p; });

  // ── SCENE 1 · New agent (3–9.6s) ── (no right panel while drafting; center pane is wide, composer ~x776)
  tw(3300, 320, E.out, p => { S.cur.op = p; });
  tw(3300, 1300, E.settle, p => { S.cam.s = lerp(1, 1.34, p); S.cam.fx = lerp(W / 2, 776, p); S.cam.fy = lerp(H / 2, 470, p); }); // push to composer
  tw(3500, 360, E.inout, p => { S.cur.x = lerp(TGT.compRest.x + 40, TGT.compRest.x, p); S.cur.y = lerp(TGT.compRest.y - 14, TGT.compRest.y, p); });
  tw(3650, 2500, E.linear, p => { S.typed = p; });                         // type the prompt
  tw(4000, 950, E.settle, p => { S.o[0].op = p; S.o[0].ty = lerp(26, 0, p); S.o[0].bl = lerp(8, 0, p); });
  tw(6100, 600, E.soft, p => { S.o[0].op = 1 - p; S.o[0].ty = lerp(0, -16, p); S.o[0].bl = lerp(0, 6, p); }); // out before the provider-reveal cut
  // cursor to the provider chip, then the "works with every agent" reveal cut
  tw(6150, 450, E.inout, p => { S.cur.x = lerp(TGT.compRest.x, TGT.prov.x, p); S.cur.y = lerp(TGT.compRest.y, TGT.prov.y, p); });
  tw(6600, 360, E.out, p => { S.ring = p; });
  tw(6700, 300, E.soft, p => { S.cur.op = 1 - p; });                       // hide cursor behind the cut
  tw(6700, 420, E.soft, p => { S.prReveal = p; });                         // cut to interstitial
  tw(7050, 460, E.inout, p => { S.cur.x = lerp(TGT.prov.x, TGT.effort.x, p); S.cur.y = lerp(TGT.prov.y, TGT.effort.y, p); }); // glide to effort while hidden
  PROVIDERS.forEach((_, i) => {
    const st = 6900 + (PROVIDERS.length - 1 - i) * 95;                     // right→left cascade
    tw(st, 620, E.back, p => { S.prAgent[i] = p; });
  });
  tw(8350, 480, E.soft, p => { S.prReveal = 1 - p; });                     // cut back to composer
  // cursor to the effort chip, click through Low → Med → High → xHigh (label cycles per click)
  tw(8450, 300, E.out, p => { S.cur.op = p; });                            // reappear, already on the effort chip
  tw(8560, 220, E.out, p => { S.ring = p; }); tw(8560, 220, E.out, p => { S.effortPress = Math.sin(p * Math.PI); });
  tw(8810, 220, E.out, p => { S.ring = p; }); tw(8810, 220, E.out, p => { S.effortPress = Math.sin(p * Math.PI); });
  tw(9060, 220, E.out, p => { S.ring = p; }); tw(9060, 220, E.out, p => { S.effortPress = Math.sin(p * Math.PI); });
  // cursor to send, press
  tw(9280, 360, E.inout, p => { S.cur.x = lerp(TGT.effort.x, TGT.send.x, p); S.cur.y = lerp(TGT.effort.y, TGT.send.y, p); });
  tw(9640, 200, E.out, p => { S.ring = p; });
  tw(9680, 260, E.out, p => { S.sendPress = Math.sin(p * Math.PI); });
  // transition to the workspace
  tw(9760, 520, E.soft, p => { S.newOp = 1 - p; });
  tw(9800, 620, E.settle, p => { S.workOp = p; });
  tw(9800, 420, E.soft, p => { S.cur.op = 1 - p; });

  // ── SCENE 2 · Sidebar fleet (9.6–15.2s) ──
  tw(9900, 1300, E.settle, p => { S.cam.s = lerp(1.34, 1.62, p); S.cam.fx = lerp(776, 175, p); S.cam.fy = lerp(470, 250, p); }); // composer → sidebar top
  tw(10650, 850, E.out, p => { S.heroTask = p; });                         // hero task wipes in
  tw(10700, 950, E.settle, p => { S.o[1].op = p; S.o[1].ty = lerp(26, 0, p); S.o[1].bl = lerp(8, 0, p); });
  tw(14200, 600, E.soft, p => { S.o[1].op = 1 - p; S.o[1].ty = lerp(0, -16, p); S.o[1].bl = lerp(0, 6, p); }); // out near scene end
  tw(12000, 1700, E.inout, p => { S.cam.s = lerp(1.62, 1.3, p); S.cam.fx = lerp(175, 200, p); S.cam.fy = lerp(250, 380, p); }); // ease back to reveal the stack
  // each agent pops in at its own irregular spawn time (top-down, natural cadence)
  AGENTS.forEach((a, i) => { tw(a.spawn, 560, E.back, p => { S.agentPop[i] = p; }); });

  // ── SCENE 3 · Chat (15.2–22.6s) ──
  tw(15200, 1300, E.settle, p => { S.cam.s = lerp(1.34, 1.46, p); S.cam.fx = lerp(200, 560, p); S.cam.fy = lerp(330, 250, p); }); // pan into chat
  tw(15700, 400, E.out, p => { S.thinkOp = p; });
  tw(16200, 950, E.settle, p => { S.o[2].op = p; S.o[2].ty = lerp(26, 0, p); S.o[2].bl = lerp(8, 0, p); });
  tw(21600, 600, E.soft, p => { S.o[2].op = 1 - p; S.o[2].ty = lerp(0, -16, p); S.o[2].bl = lerp(0, 6, p); }); // out near scene end
  tw(16300, 1500, E.linear, p => { S.reason = p; });                       // reasoning types in
  tw(16500, 500, E.soft, p => { S.thinkOp = 1 - p; });                     // thinking fades as reasoning starts
  TOOLS.forEach((_, i) => {                                                // each tool: icon → name → arg → result
    const st = 17600 + i * 900;
    tw(st, 380, E.back, p => { S.tIco[i] = p; });
    tw(st + 150, 460, E.settle, p => { S.tName[i] = p; });
    tw(st + 280, 420, E.soft, p => { S.tArg[i] = p; });
    tw(st + 480, 360, E.soft, p => { S.tRes[i] = p; });
  });
  [0, 1, 2].forEach(i => { const st = 21200 + i * 320; tw(st, 620, E.settle, p => { S.resp[i] = p; }); });
  tw(16800, 5200, E.inout, p => { S.chatScroll = lerp(0, -150, p); });      // gentle auto-scroll

  // ── SCENE 4 · Live code (22.6–27.6s) ──
  tw(22600, 1300, E.settle, p => { S.cam.s = lerp(1.46, 1.42, p); S.cam.fx = lerp(560, 1088, p); S.cam.fy = lerp(250, 300, p); }); // pan to code panel
  [0, 1, 2].forEach(i => { const st = 22900 + i * 110; tw(st, 460, E.back, p => { S.codeTab[i] = p; }); });
  tw(23200, 950, E.settle, p => { S.o[3].op = p; S.o[3].ty = lerp(26, 0, p); S.o[3].bl = lerp(8, 0, p); });
  tw(26600, 600, E.soft, p => { S.o[3].op = 1 - p; S.o[3].ty = lerp(0, -16, p); S.o[3].bl = lerp(0, 6, p); }); // out near scene end
  // each file's diff streams in while it's the active file (panel content switches across files)
  tw(23050, 1300, E.linear, p => { S.fileStream[0] = p * FILES[0].lines.length; });
  tw(24680, 850,  E.linear, p => { S.fileStream[1] = p * FILES[1].lines.length; });
  tw(25760, 1150, E.linear, p => { S.fileStream[2] = p * FILES[2].lines.length; });

  // ── SCENE 5 · Git / ship (27.6–33.8s) ──
  tw(27600, 520, E.soft, p => { S.codeOp = 1 - p; });                       // crossfade code → git
  tw(27680, 560, E.settle, p => { S.gitOp = p; });
  tw(27750, 1100, E.settle, p => { S.cam.s = lerp(1.42, 1.5, p); S.cam.fx = lerp(1088, 1090, p); S.cam.fy = lerp(300, 540, p); }); // frame the action bar
  [0, 1, 2].forEach(i => { const st = 28000 + i * 110; tw(st, 480, E.back, p => { S.gitFile[i] = p; }); });
  tw(28300, 950, E.settle, p => { S.o[4].op = p; S.o[4].ty = lerp(26, 0, p); S.o[4].bl = lerp(8, 0, p); });
  tw(32900, 600, E.soft, p => { S.o[4].op = 1 - p; S.o[4].ty = lerp(0, -16, p); S.o[4].bl = lerp(0, 6, p); }); // out through the merge, before camera leaves
  tw(28650, 380, E.out, p => { S.cur.op = p; S.cur.x = TGT.gitBtn.x; S.cur.y = lerp(TGT.gitBtn.y - 56, TGT.gitBtn.y, p); }); // cursor to the CTA
  tw(28850, 220, E.out, p => { S.ring = p; });                             // commit click
  tw(29950, 220, E.out, p => { S.ring = p; });                             // push click
  tw(30600, 620, E.settle, p => { S.prCard = p; });                        // PR card blooms
  tw(32700, 220, E.out, p => { S.ring = p; });                             // merge click
  tw(33450, 320, E.soft, p => { S.cur.op = 1 - p; });                      // cursor leaves after merge

  // ── SCENE 6 · Idle + recall (33.8–38s) ──
  tw(33900, 1200, E.settle, p => { S.cam.s = lerp(1.5, 1.62, p); S.cam.fx = lerp(1090, 175, p); S.cam.fy = lerp(540, 235, p); }); // back to hero
  tw(34350, 700, E.back, p => { S.heroBadge = p; });                       // merged badge blooms (status flips at 34200)
  tw(34500, 950, E.settle, p => { S.o[5].op = p; S.o[5].ty = lerp(26, 0, p); S.o[5].bl = lerp(8, 0, p); });
  tw(36800, 600, E.soft, p => { S.o[5].op = 1 - p; S.o[5].ty = lerp(0, -16, p); S.o[5].bl = lerp(0, 6, p); }); // out before the archive click
  tw(34800, 420, E.out, p => { S.cur.op = p; S.cur.x = lerp(TGT.heroTime.x - 28, TGT.heroTime.x, p); S.cur.y = lerp(TGT.heroTime.y + 26, TGT.heroTime.y, p); }); // hover the time → stats
  tw(34950, 700, E.settle, p => { S.heroStats = p; });                     // stats popover
  tw(36800, 320, E.soft, p => { S.heroStats = 1 - p; });
  tw(37000, 380, E.inout, p => { S.cur.x = lerp(TGT.heroTime.x, TGT.heroArch.x, p); S.cur.y = lerp(TGT.heroTime.y, TGT.heroArch.y, p); }); // up to the archive button
  tw(37420, 220, E.out, p => { S.ring = p; });                             // click archive
  tw(37520, 600, E.settle, p => { S.heroCollapse = p; });                  // row collapses out
  tw(37680, 360, E.soft, p => { S.cur.op = 1 - p; });

  // ── SCENE 7 · Close (38–42s) ──
  tw(38000, 1900, E.inout, p => { S.cam.s = lerp(1.62, 1, p); S.cam.fx = lerp(175, W / 2, p); S.cam.fy = lerp(235, H / 2, p); }); // slow push out
  tw(39200, 1000, E.soft, p => { S.scrim = lerp(0, .7, p); });
  tw(39500, 1100, E.settle, p => { S.close.op = p; S.close.ty = lerp(28, 0, p); S.close.bl = lerp(10, 0, p); });

  T.sort((a, b) => a.t - b.t);

  /* ---------- elements ---------- */
  const $ = id => document.getElementById(id);
  const el = {
    app: $("app"), stage: $("stage"), appE: $("appEntrance"), cam: $("camera"),
    brand: $("brand"), brandTag: $("brandTag"),
    screenNew: $("screenNew"), screenWork: $("screenWork"),
    naText: $("naText"), naCaret: $("naCaret"), naSend: $("naSend"), naPh: $("naPh"),
    effortChip: $("effortChip"), effortLbl: $("effortLbl"),
    provReveal: $("provReveal"), prRow: $("prRow"),
    crumbAgent: $("crumbAgent"),
    agentList: $("agentList"), projCount: $("projCount"),
    thinking: $("thinking"), reasoning: $("reasoning"), reasoningText: $("reasoningText"),
    response: $("response"), chatInner: $("chatInner"),
    tabCode: $("tabCode"), tabGit: $("tabGit"),
    codePanel: $("codePanel"), gitPanel: $("gitPanel"),
    codeTabs: $("codeTabs"), diff: $("diff"),
    gitFiles: $("gitFiles"), prCard: $("prCard"), gitHdr: $("gitHdr"), gitPill: $("gitPill"),
    gitStatus: $("gitStatus"), gitStatusLbl: $("gitStatusLbl"),
    gitSplit: $("gitSplit"), gitMain: $("gitMain"), gitMainIco: $("gitMainIco"), gitMainLbl: $("gitMainLbl"),
    scrim: $("scrim"), closeTitle: $("closeTitle"),
    cursor: $("cursor"), ring: $("clickRing"),
    o: [$("o1"), $("o2"), $("o3"), $("o4"), $("o5"), $("o6")],
    tool: [$("tool0"), $("tool1"), $("tool2"), $("tool3")],
    resp: [$("resp0"), $("resp1"), $("resp2")],
    chk: [$("chk0"), $("chk1"), $("chk2")],
  };

  /* ---------- build dynamic DOM ---------- */
  const SVGS = window.PROVIDER_SVG || {};
  // hue-tinted brand-icon chip (real agent SVG), mirroring src/components/ProviderIcon.tsx
  function chip(slug, hue, size) {
    const r = Math.max(3, Math.round(size * 0.233));
    return `<span class="chip-mono" style="width:${size}px;height:${size}px;border-radius:${r}px;--ph-h:${hue};--ph:oklch(.65 .13 ${hue})"><span class="chip-mono-svg">${SVGS[slug] || ""}</span></span>`;
  }
  // populate the static brand-icon chips + effort sparkle
  function setChip(id, slug, hue) {
    const e = $(id); if (!e) return;
    e.style.setProperty("--ph-h", hue); e.style.setProperty("--ph", `oklch(.65 .13 ${hue})`);
    e.innerHTML = `<span class="chip-mono-svg">${SVGS[slug] || ""}</span>`;
  }
  setChip("provChipIco", "claude", 28);
  setChip("wsProvIco", "claude", 28);
  setChip("draftProvIco", "claude", 28);
  const effSpark = $("effortSpark"); if (effSpark) effSpark.innerHTML = ICONS.sparkle;
  // provider reveal row
  PROVIDERS.forEach(p => {
    const d = document.createElement("div");
    d.className = "pr-agent";
    d.innerHTML = `<span class="pr-ico" style="--ph-h:${p.hue};--ph:oklch(.65 .13 ${p.hue})"><span class="chip-mono-svg">${SVGS[p.slug] || ""}</span></span><span class="pr-name">${p.name}</span>`;
    el.prRow.appendChild(d);
  });
  const prAgentEls = Array.from(el.prRow.children);
  // sidebar agents — one unified row; state (rail/loader/badge) is set per-frame in render
  const AG = [];
  AGENTS.forEach((a, i) => {
    const d = document.createElement("div");
    d.className = "agent" + (a.hero ? " active" : "");
    d.style.setProperty("--ag-sync", a.sync + "s"); // independent loader/shimmer clock per agent
    d.dataset.i = i;
    d.innerHTML =
      `<span class="ag-rail"></span>
       <div class="agent-row">
         <span class="ag-name">${a.name}</span>
         <span class="ag-prov-chip">${chip(a.slug, a.hue, 14)}</span>
         <span class="ag-slot">
           <span class="ag-meta"><span class="ag-loader"></span></span>
           <span class="ag-actions">
             <button class="ag-act ag-stop">${ICONS.stop}</button>
             <button class="ag-act ag-arch">${ICONS.archive}</button>
           </span>
         </span>
       </div>
       <div class="agent-sub"><span class="a-task">${a.task}</span><span class="a-meta"></span><span class="a-time">${a.age}</span></div>`;
    el.agentList.appendChild(d);
    const r = {
      root: d, rail: d.querySelector(".ag-rail"), name: d.querySelector(".ag-name"),
      loader: d.querySelector(".ag-loader"), stop: d.querySelector(".ag-stop"), arch: d.querySelector(".ag-arch"),
      aMeta: d.querySelector(".a-meta"), time: d.querySelector(".a-time"), _status: null,
    };
    if (a.hero) {
      const pop = document.createElement("div");
      pop.className = "ag-stats-pop"; pop.style.opacity = "0"; pop.style.display = "none";
      pop.innerHTML =
        `<div class="st-row"><span class="st-k">Launched</span><span class="st-v">just now</span></div>
         <div class="st-row"><span class="st-k">Runtime</span><span class="st-v">4m 12s</span></div>
         <div class="st-row"><span class="st-k">Last turn</span><span class="st-v">84.2k / 200k</span></div>
         <div class="st-bar"><div class="st-bar-fill" style="width:42%"></div></div>
         <div class="st-row"><span class="st-k">Context used</span><span class="st-v">42%</span></div>`;
      d.appendChild(pop);
      r.stats = pop;
    }
    AG.push(r);
  });
  el.heroAgent = AG[0].root; el.heroTime = AG[0].time; el.heroArch = AG[0].arch;
  el.heroStats = AG[0].stats; el.heroTask = AG[0].root.querySelector(".a-task");
  el._chk = [-1, -1, -1]; el._mainLbl = null; el._tone = null; el._sLbl = null; // change caches for git DOM
  // diff blocks — one per file, stacked and crossfaded by the active file
  el.diff.innerHTML = "";
  const fileEls = FILES.map(f => {
    const wrap = document.createElement("div");
    wrap.className = "code-file";
    let html = `<div class="code-hunk-h">${f.hunk}</div>`;
    f.lines.forEach((l, i) => {
      const sig = l.op === "add" ? "+" : l.op === "rem" ? "−" : " ";
      html += `<div class="dl op-${l.op}"><span class="dl-num">${i + 1}</span><span class="dl-sigil">${sig}</span><span class="dl-text">${l.t.replace(/</g, "&lt;") || " "}</span></div>`;
    });
    wrap.innerHTML = html;
    el.diff.appendChild(wrap);
    return wrap;
  });
  const fileLineEls = fileEls.map(w => Array.from(w.querySelectorAll(".dl")));
  // tool rows content
  TOOLS.forEach((t, i) => {
    el.tool[i].innerHTML = `<span class="t-icon">${ICONS[t.ic]}</span><span class="t-name">${t.name}</span><span class="t-arg">${t.arg}</span><span class="t-result">${t.res}</span>`;
  });

  /* ---------- layout measure ---------- */
  let stageScale = 1;
  // Real app-space centers of the elements the cursor clicks. Measured from the
  // live layout (offsetLeft/Top chain up to #app), so the cursor lands exactly on
  // each target after the camera projection — no hand-tuned guesses.
  const TGT = {};
  function appCenter(node) {
    let x = 0, y = 0, n = node;
    while (n && n !== el.app) { x += n.offsetLeft; y += n.offsetTop; n = n.offsetParent; }
    return { x: x + node.offsetWidth / 2, y: y + node.offsetHeight / 2 };
  }
  function measure() {
    // git/code panels are display-toggled at runtime; force layout so we can measure them
    const gp = el.gitPanel.style.display, cp = el.codePanel.style.display;
    el.gitPanel.style.display = "flex"; el.codePanel.style.display = "flex";
    TGT.prov = appCenter($("provChip"));
    TGT.effort = appCenter($("effortChip"));
    TGT.send = appCenter($("naSend"));
    TGT.gitBtn = appCenter(el.gitMain);
    TGT.heroTime = appCenter(el.heroTime);
    TGT.heroRow = appCenter(el.heroAgent);
    // archive button center, measured as it'll be in scene 6 (idle row → stop hidden)
    const ds = AG[0].stop.style.display; AG[0].stop.style.display = "none";
    TGT.heroArch = appCenter(el.heroArch);
    AG[0].stop.style.display = ds;
    el.gitPanel.style.display = gp; el.codePanel.style.display = cp;
    TGT.compRest = { x: (TGT.prov.x + TGT.send.x) / 2, y: TGT.prov.y - 34 };
  }

  /* ---------- git action state machine (discrete, time-driven) ---------- */
  function gitState(t) {
    // returns {mainLbl, mainIco, tone, statusLbl, statusKind, busy, checks:[0=idle,1=spin,2=ok], merged}
    const g = { mainLbl: "Commit", mainIco: ICONS.commit, tone: "", statusLbl: "3 files changed", statusKind: "", busy: false, checks: [0, 0, 0], merged: false };
    if (t >= 28850 && t < 29350) { g.busy = true; g.statusLbl = "Committing…"; }
    if (t >= 29350) { g.mainLbl = "Push"; g.mainIco = ICONS.push; g.statusLbl = "1 commit ahead"; }
    if (t >= 29950 && t < 30550) { g.busy = true; g.statusLbl = "Pushing to origin…"; }
    if (t >= 30550) { g.mainLbl = "Merge"; g.mainIco = ICONS.merge; g.statusLbl = "Checks running…"; g.statusKind = "info"; g.tone = "ghost"; }
    // checks resolve one-by-one
    const cs = [[30700, 31300], [30700, 31850], [30700, 32350]];
    if (t >= 30550) cs.forEach((c, i) => { g.checks[i] = t < c[0] ? 0 : t < c[1] ? 1 : 2; });
    if (t >= 32350) { g.statusLbl = "All checks passed"; g.statusKind = "ready"; g.tone = "success"; }
    if (t >= 32700 && t < 33300) { g.busy = true; g.statusLbl = "Merging…"; g.tone = "success"; }
    if (t >= 33300) { g.mainLbl = "Merged"; g.mainIco = ICONS.merge; g.tone = "merged"; g.statusLbl = "Merged into main"; g.statusKind = "merged"; g.merged = true; }
    return g;
  }

  /* ---------- render ---------- */
  function render(time) {
    S = base();
    for (const k of T) { if (time < k.t) continue; const p = k.d > 0 ? Math.min(1, (time - k.t) / k.d) : 1; k.fn(k.e(p), p); }

    // camera
    const c = S.cam, tx = W / 2 - c.fx * c.s, ty = H / 2 - c.fy * c.s;
    el.cam.style.transform = `translate(${tx}px,${ty}px) scale(${c.s})`;
    el.appE.style.opacity = S.appOp;
    el.appE.style.transform = `scale(${S.appScale})`;

    // brand cold open
    el.brand.style.opacity = S.brand.op;
    el.brand.style.transform = `translateY(-50%) translateY(${S.brand.ty}px)`;
    el.brand.style.filter = `blur(${S.brand.bl}px)`;
    el.brandTag.style.opacity = S.brandTag.op;
    el.brandTag.style.transform = `translateY(${S.brandTag.ty}px)`;

    // screen crossfade
    el.screenNew.style.opacity = S.newOp;
    el.screenNew.style.pointerEvents = "none";
    el.screenWork.style.opacity = S.workOp;
    el.crumbAgent.textContent = "dolomites";

    // SCENE 1 — typed prompt
    const n = Math.round(S.typed * PROMPT.length);
    el.naText.textContent = PROMPT.slice(0, n);
    if (el.naPh) el.naPh.style.display = S.typed > 0 ? "none" : "";
    const typing = S.typed > 0 && S.typed < 1;
    el.naCaret.style.opacity = S.newOp * (typing ? (Math.floor(time / 480) % 2 ? .3 : 1) : 0);
    // effort chip — single cycling chip (sparkle + level), label advances per click
    el.effortLbl.textContent = time < 8560 ? "Low" : time < 8810 ? "Med" : time < 9060 ? "High" : "xHigh";
    el.effortChip.style.transform = `scale(${1 - S.effortPress * .08})`;
    el.naSend.style.transform = `scale(${1 - S.sendPress * .12})`;
    // provider reveal interstitial
    el.provReveal.style.opacity = S.prReveal;
    prAgentEls.forEach((d, i) => {
      const v = S.prAgent[i];
      d.style.opacity = v;
      d.style.transform = `translateX(${lerp(220, 0, v)}px) scale(${lerp(.9, 1, v)})`;
    });

    // SCENE 2/6 — sidebar agents: pop-in + per-agent live state (rail/loader/badge)
    let shown = 0;
    AG.forEach((r, i) => {
      const a = AGENTS[i];
      const pop = S.agentPop[i];
      if (pop > .02) shown++;
      r.root.style.opacity = pop;
      // drop the transform once settled — a lingering scale(1) makes a stacking
      // context that would trap the hero's stats popover behind later rows.
      r.root.style.transform = pop >= .999 ? "none" : `translateY(${lerp(10, 0, pop)}px) scale(${lerp(.92, 1, pop)})`;
      if (pop < .02) return;
      const status =
        a.hero ? (time >= 34200 ? "merged" : "running") :
        a.merged ? "merged" :
        (a.idleAt && time >= a.idleAt) ? "idle" :
        (a.waitAt && time >= a.waitAt) ? "waiting" : "running";
      if (r._status !== status) {
        r.rail.className = "ag-rail " + (status === "running" ? "run" : status === "merged" ? "merged" : status === "waiting" ? "wait" : "idle");
        r.name.classList.toggle("shimmer", status === "running");
        r.loader.style.display = status === "running" ? "" : "none";
        r.stop.style.display = status === "running" ? "" : "none";
        r.arch.style.display = (status === "idle" || status === "merged") ? "" : "none";
        if (status === "merged") { r.aMeta.className = "ag-badge pr-merged"; r.aMeta.innerHTML = `${ICONS.merge}#${a.hero ? "147" : a.merged}`; }
        else if (status === "waiting") { r.aMeta.className = "ag-badge wait"; r.aMeta.innerHTML = `${ICONS.help}waiting`; }
        else if (a.pr === "open") { r.aMeta.className = "ag-badge pr-open"; r.aMeta.innerHTML = `${ICONS.pr}PR`; }
        else { r.aMeta.className = "a-diff"; r.aMeta.innerHTML = `<span class="add">+${a.add}</span> <span class="del">−${a.del}</span>`; }
        r._status = status;
      }
    });
    if (S.heroCollapse > 0.5) shown--;                                     // hero archived → drops out of the count
    el.projCount.textContent = shown;
    // hero: task wipe (scene 2), merged-badge bloom + stats popover + hover/archive (scene 6)
    el.heroTask.style.clipPath = `inset(0 ${(1 - S.heroTask) * 100}% 0 0)`;
    el.heroTask.style.opacity = clamp01(S.heroTask * 1.4);
    if (time >= 34200) AG[0].aMeta.style.transform = `scale(${lerp(.8, 1, S.heroBadge)})`;
    el.heroStats.style.opacity = S.heroStats;
    el.heroStats.style.transform = `translateY(${lerp(-6, 0, S.heroStats)}px) scale(${lerp(.96, 1, S.heroStats)})`;
    el.heroStats.style.display = S.heroStats > .01 ? "" : "none";
    el.heroTime.classList.toggle("lit", S.heroStats > .01);
    el.heroAgent.style.zIndex = S.heroStats > .01 ? "100" : "";
    el.heroAgent.classList.toggle("hovering", time >= 34880 && time < 37820); // reveal the archive action
    if (S.heroCollapse > 0) {
      el.heroAgent.style.maxHeight = lerp(58, 0, S.heroCollapse) + "px";
      el.heroAgent.style.opacity = 1 - S.heroCollapse;
      el.heroAgent.style.overflow = "hidden";
      el.heroAgent.style.transform = `translateX(${lerp(0, -16, S.heroCollapse)}px)`;
    } else {
      el.heroAgent.style.maxHeight = "";
    }

    // SCENE 3 — chat
    el.thinking.style.opacity = S.thinkOp;
    el.thinking.style.display = S.thinkOp > .01 ? "" : "none";
    const rn = Math.round(S.reason * REASON.length);
    el.reasoningText.textContent = REASON.slice(0, rn);
    el.reasoning.style.opacity = S.reason > 0 ? 1 : 0;
    el.reasoning.style.display = S.reason > 0 ? "" : "none";
    el.tool.forEach((d, i) => {
      const anyVis = S.tIco[i] > .01;
      d.style.display = anyVis ? "" : "none";
      const ic = d.querySelector(".t-icon"), nm = d.querySelector(".t-name"), ar = d.querySelector(".t-arg"), rs = d.querySelector(".t-result");
      if (ic) { ic.style.opacity = S.tIco[i]; ic.style.transform = `scale(${lerp(.3, 1, S.tIco[i])}) rotate(${lerp(-18, 0, S.tIco[i])}deg)`; }
      if (nm) { nm.style.opacity = S.tName[i]; nm.style.transform = `translateX(${lerp(-8, 0, S.tName[i])}px)`; }
      if (ar) ar.style.opacity = S.tArg[i];
      if (rs) rs.style.opacity = S.tRes[i];
    });
    let respVis = false;
    el.resp.forEach((d, i) => { d.style.opacity = S.resp[i]; d.style.transform = `translateY(${lerp(8, 0, S.resp[i])}px)`; if (S.resp[i] > .01) respVis = true; });
    el.response.style.display = respVis ? "" : "none";
    el.chatInner.style.transform = `translateY(${S.chatScroll}px)`;

    // SCENE 4 — code panel
    el.codePanel.style.opacity = S.codeOp;
    el.codePanel.style.display = S.codeOp > .01 ? "" : "none";
    // active file switches as the agent works across the three files
    const activeFile = time < 24600 ? 0 : time < 25700 ? 1 : 2;
    el.codeTabs.querySelectorAll(".code-tab").forEach((d, i) => {
      d.style.opacity = S.codeTab[i];
      d.style.transform = S.codeTab[i] >= .999 ? "none" : `translateY(${lerp(6, 0, S.codeTab[i])}px) scale(${lerp(.96, 1, S.codeTab[i])})`;
      d.classList.toggle("active", i === activeFile && time >= 22900 && time < 27600);
    });
    fileEls.forEach((w, i) => { w.style.opacity = (i === activeFile && time >= 22900) ? 1 : 0; });
    fileLineEls.forEach((lines, fi) => {
      lines.forEach((d, i) => { const p = clamp01(S.fileStream[fi] - i); d.style.opacity = p; d.style.transform = `translateY(${lerp(6, 0, p)}px)`; });
    });

    // tab active state
    const gitActive = time >= 27600;
    el.tabCode.classList.toggle("active", !gitActive);
    el.tabGit.classList.toggle("active", gitActive);

    // SCENE 5 — git panel
    el.gitPanel.style.opacity = S.gitOp;
    el.gitPanel.style.display = S.gitOp > .01 ? "" : "none";
    el.gitFiles.querySelectorAll(".git-file").forEach((d, i) => {
      d.style.opacity = S.gitFile[i];
      d.style.transform = `translateX(${lerp(-10, 0, S.gitFile[i])}px)`;
    });
    el.prCard.style.opacity = S.prCard;
    el.prCard.style.transform = `translateY(${lerp(10, 0, S.prCard)}px)`;
    el.prCard.style.display = S.prCard > .01 ? "" : "none";
    const g = gitState(time);
    if (el._mainLbl !== g.mainLbl) { el.gitMain.innerHTML = g.mainIco + `<span>${g.mainLbl}</span>`; el._mainLbl = g.mainLbl; }
    if (el._tone !== g.tone) { el.gitSplit.className = "git-split" + (g.tone ? " " + g.tone : ""); el._tone = g.tone; }
    el.gitStatus.className = "git-act-status" + (g.statusKind ? " " + g.statusKind : "");
    const sLbl = (g.busy ? '<span class="git-spin"></span> ' : "") + g.statusLbl;
    if (el._sLbl !== sLbl) { el.gitStatusLbl.innerHTML = sLbl; el._sLbl = sLbl; }
    el.gitHdr.classList.toggle("k-merged", g.merged);
    el.gitPill.textContent = g.merged ? "merged" : "3 changes";
    el.chk.forEach((cel, i) => {
      const st = g.checks[i];
      if (el._chk[i] === st) return;
      cel.parentElement.classList.toggle("ok", st === 2);
      cel.innerHTML = st === 2 ? ICONS.check : st === 1 ? '<span class="pc-spin"></span>' : '<span style="width:6px;height:6px;border-radius:50%;background:var(--fg-3);display:inline-block"></span>';
      el._chk[i] = st;
    });

    // overlays
    el.o.forEach((d, i) => { const s = S.o[i]; d.style.opacity = s.op; d.style.transform = `translateY(${s.ty}px)`; d.style.filter = `blur(${s.bl}px)`; });
    el.scrim.style.opacity = S.scrim;
    el.closeTitle.style.opacity = S.close.op;
    el.closeTitle.style.transform = `translateY(${S.close.ty}px)`;
    el.closeTitle.style.filter = `blur(${S.close.bl}px)`;

    // cursor + ring — cursor targets are app-space coords, projected through the
    // camera so they sit on the zoomed UI while staying a fixed 24px on screen.
    const csx = tx + S.cur.x * c.s, csy = ty + S.cur.y * c.s;
    el.cursor.style.opacity = S.cur.op;
    el.cursor.style.transform = `translate(${csx}px,${csy}px)`;
    el.ring.style.left = csx + "px"; el.ring.style.top = csy + "px";
    el.ring.style.opacity = S.ring > 0 ? (1 - S.ring) : 0;
    el.ring.style.transform = `translate(-50%,-50%) scale(${S.ring * 1.4})`;
  }

  /* ---------- scaler ---------- */
  function fit() {
    stageScale = Math.min(window.innerWidth / (W + 80), window.innerHeight / (H + 80));
    el.stage.style.transform = `translate(-50%,-50%) scale(${stageScale})`;
    measure();
  }
  window.addEventListener("resize", fit);

  /* ---------- playback ---------- */
  let playing = false, cur = 0, last = 0, raf = null;
  const ui = $("ui"), fill = $("fill"), track = $("track"), timeEl = $("time"), playIcon = $("playIcon"), hint = $("hint");
  const PLAY = "M8 5v14l11-7z", PAUSE = "M6 5h4v14H6zM14 5h4v14h-4z";
  function mmss(ms) { const s = Math.floor(ms / 1000); return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0"); }
  function syncUI() { fill.style.width = (cur / DUR * 100) + "%"; timeEl.textContent = mmss(cur) + " / 0:42"; playIcon.setAttribute("d", playing ? PAUSE : PLAY); }
  function loop(ts) {
    if (!playing) return;
    if (!last) last = ts;
    cur += ts - last; last = ts;
    if (cur >= DUR) { cur = DUR; playing = false; render(cur); syncUI(); showUI(true); return; }
    render(cur); syncUI();
    raf = requestAnimationFrame(loop);
  }
  function play() { if (cur >= DUR) cur = 0; playing = true; last = 0; showUI(false); raf = requestAnimationFrame(loop); syncUI(); }
  function pause() { playing = false; if (raf) cancelAnimationFrame(raf); showUI(true); syncUI(); }
  function toggle() { playing ? pause() : play(); }
  function replay() { cur = 0; render(0); play(); }
  function seek(ms) { cur = Math.max(0, Math.min(DUR, ms)); render(cur); syncUI(); }
  [0, 2600, 9600, 15200, 22600, 27600, 33800, 38000].forEach(t => { const d = document.createElement("div"); d.className = "tick"; d.style.left = (t / DUR * 100) + "%"; track.appendChild(d); });
  let dragging = false;
  const trackToMs = e => { const r = track.getBoundingClientRect(); return Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)) * DUR; };
  track.addEventListener("pointerdown", e => { dragging = true; pause(); seek(trackToMs(e)); track.setPointerCapture(e.pointerId); });
  track.addEventListener("pointermove", e => { if (dragging) seek(trackToMs(e)); });
  track.addEventListener("pointerup", () => { dragging = false; });
  $("playBtn").onclick = toggle;
  $("replayBtn").onclick = replay;
  $("fsBtn").onclick = () => { if (!document.fullscreenElement) document.documentElement.requestFullscreen && document.documentElement.requestFullscreen(); else document.exitFullscreen && document.exitFullscreen(); };
  let hideT = null;
  function showUI(force) {
    ui.classList.remove("hide"); hint.classList.remove("hide");
    clearTimeout(hideT);
    if (!force && playing) hideT = setTimeout(() => { if (playing) { ui.classList.add("hide"); hint.classList.add("hide"); } }, 2600);
  }
  window.addEventListener("mousemove", () => { if (playing) showUI(false); });
  document.addEventListener("keydown", e => {
    if (e.code === "Space") { e.preventDefault(); toggle(); }
    else if (e.key.toLowerCase() === "r") replay();
    else if (e.key.toLowerCase() === "h") { ui.classList.toggle("hide"); hint.classList.toggle("hide"); }
    else if (e.key.toLowerCase() === "f") $("fsBtn").click();
  });

  /* ---------- boot ---------- */
  fit(); render(0); syncUI();
  // ?t=<ms> renders a single static frame (for tuning / frame capture) and skips autoplay.
  const seek0 = new URLSearchParams(location.search).get("t");
  if (seek0 !== null) { setTimeout(() => { measure(); cur = Math.max(0, Math.min(DUR, +seek0)); render(cur); syncUI(); }, 120); }
  else setTimeout(() => { measure(); play(); }, 500);
})();
