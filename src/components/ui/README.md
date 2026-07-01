# UI primitives

Shared, presentational building blocks. **Reach for these instead of
re-applying the underlying CSS classes by hand** — it keeps styling consistent
and is the path of least resistance for new UI.

| Primitive | Use for | Underlying class |
|---|---|---|
| `Badge` | Compact status pill — agent state (`new`/`err`) and PR state (`pr-open`/`pr-merged`/`pr-closed`). Non-interactive. | `.ag-badge` |
| `Button` | Text-label button — CTAs and dialog actions (Cancel / Save / Restart). Variants `ghost` / `outline` / `primary` (+ `danger`), `size="sm"`. | `.btn-t` |
| `IconButton` | Square icon-only button (title bar, sidebar, panels). Built-in CSS tooltip via `tip`. | `.btn-i` |
| `Loader` | Three-dot bounce loader — working / pending / restoring states. Variants `accent` / `muted` / `inherit`, `size="sm"` / `"md"`. | `.ui-loader` |
| `Chip` | Composer footer chip with a text label (model picker, base branch, attach). | `.c-chip` |
| `Select` | Custom `<select>` replacement (keyboard-operable dropdown of string options). | `.ui-select-*` |
| `DropdownMenu` / `DropdownItem` / `DropdownSection` / `DropdownSeparator` | Presentational menu shell + rows. Owns structure + state classes (`active`/`disabled`/`danger`); **caller owns behavior** (open/close, positioning via `style`, dismissal, keyboard). | `.dd` / `.dd-item` |
| `CopyButton` | Copy-to-clipboard affordance with copied-state feedback. | — |
| `Scrim` | Full-screen dim/click-catcher behind popovers and overlays. | — |

Tooltips are CSS-only: pass `tip="…"` (and `tipDown` where supported) — it sets
`.tip` + `data-tip` and renders on hover. No JS tooltip library.

## Conventions
- Each primitive is a thin wrapper: a typed `Props`, a `className` passthrough,
  and a class-join. Match that shape when adding one.
- Import directly (`import { Badge } from "../ui/Badge"`) or via the barrel
  (`import { Badge } from "../ui"`).

Some menus stay bespoke on purpose — `Select` (own keyboard/focus), `ModelPicker`
(own `model-dd-*` classes + side panel), and the `@`/`#`/`/` autocomplete
(parent-controlled highlight + per-item scroll). The `Dropdown*` primitives
cover the simpler click-to-pick menus.
