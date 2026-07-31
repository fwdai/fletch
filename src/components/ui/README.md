# UI primitives

Shared, presentational building blocks. **Reach for these instead of
re-applying the underlying CSS classes by hand** — it keeps styling consistent
and is the path of least resistance for new UI.

| Primitive | Use for | Underlying class |
|---|---|---|
| `Badge` | Compact status pill — agent state (`new`/`err`) and PR state (`pr-open`/`pr-merged`/`pr-closed`). Non-interactive. | `.ag-badge` |
| `Button` | Text-label button — CTAs and dialog actions (Cancel / Save / Restart). Variants `ghost` / `outline` / `primary` / `link` / `dashed` (+ `danger`), `size="sm"` / `"lg"`. | `.btn-t` |
| `IconButton` | Square icon-only button (title bar, sidebar, panels, dialog close). `size="lg"`/`"sm"`/`"xs"`, `variant="outline"` for a bordered control beside an input, `danger` for a destructive action. Built-in CSS tooltip via `tip`. | `.btn-i` |
| `Loader` | Three-dot bounce loader — content still arriving (working / pending / restoring). Variants `accent` / `muted` / `inherit`, `size="sm"` / `"md"`. | `.ui-loader` |
| `Spinner` | Rotating-arrow busy indicator — a discrete operation the user kicked off (cloning, connecting). Inherits the caller's color. | `.ui-spin` |
| `TextInput` / `TextArea` | The app's one text-field skin — settings forms, modal forms, token/code fields. `mono` for code-ish values, `invalid` to tint the border. Text size baked in. | `.ui-input` |
| `DeviceCode` | OAuth device-flow user code + copy button + verification URL. | `.device-code` |
| `Chip` | Composer footer chip with a text label (model picker, base branch, attach). | `.c-chip` |
| `Select` | Custom `<select>` replacement (keyboard-operable dropdown of string options). | `.ui-select-*` |
| `DropdownMenu` / `DropdownItem` / `DropdownSection` / `DropdownSeparator` | Presentational menu shell + rows. Owns structure + state classes (`active`/`disabled`/`danger`); **caller owns behavior** (open/close, positioning via `style`, dismissal, keyboard). | `.dd` / `.dd-item` |
| `CopyButton` | Copy-to-clipboard affordance with copied-state feedback. | — |
| `Modal` / `ModalBody` / `ModalFooter` | Centered modal dialog — dimmed scrim + card + the standard icon/title/close header. Sizes `sm` / `md` / `lg`; `layer` picks the stacking level. The backdrop always dims; if it shouldn't, you want a popover (bare `Scrim`), not a modal. | `.modal` / `.modal-body` |
| `ModalSheet` | The wide overlay sheet (History, Project Settings) — dimmed backdrop + large card, no built-in header. `fill` pins it to full viewport height. | `.modal-sheet` |
| `Scrim` | Full-screen dim/click-catcher behind popovers and overlays. | `.ui-scrim` |

Tooltips are CSS-only: pass `tip="…"` (and `tipDown` where supported) — it sets
`.tip` + `data-tip` and renders on hover. No JS tooltip library.

## Conventions
- Each primitive is a thin wrapper: a typed `Props`, a `className` passthrough,
  and a class-join. Match that shape when adding one.
- **No hand-rolled buttons.** A clickable thing is `Button`, `IconButton`, or a
  new variant of one — never a bare `<button>` with its own skin. A feature may
  pass `className` for *layout* (margin, flex-shrink), never for appearance.
  Every `.btn-t` / `.btn-i` variant rule lives in `styles/shared/icon-button.css`
  so the size and variant classes stay in one cascade; `.btn-t.link` has to come
  after `.btn-t.sm-t` to win the specificity tie.
- Rows, cards, and tabs that happen to be `<button>` for keyboard access are not
  buttons in this sense — they keep their own classes (`.np-item`, `.np-repo`,
  `.hrow`, `.np-dest`, `.dd-item`).
- **`active` is for states that are off by default.** It paints the button with
  `var(--accent)`, which only reads as a signal if the unaccented state is the
  common one — `active={open}` on a popover trigger, not `active={!collapsed}` on
  a rail that's open by default (that's just a permanently orange button
  competing with the accents that mean something). When several call sites render
  the same control, give it a shared component rather than repeating the
  `IconButton` — see `components/PanelToggle` for the two side rails.
- Import directly (`import { Badge } from "../ui/Badge"`) or via the barrel
  (`import { Badge } from "../ui"`).

Some menus stay bespoke on purpose — `Select` (own keyboard/focus), `ModelPicker`
(own `model-dd-*` classes + side panel), and the `@`/`#`/`/` autocomplete
(parent-controlled highlight + per-item scroll). The `Dropdown*` primitives
cover the simpler click-to-pick menus.

Likewise `.rv-modal` (the approval-review surface) keeps its own shell: it's a
third card shape, already shared between its two mounts. New modals should reach
for `Modal` or `ModalSheet` rather than adding a fourth.
