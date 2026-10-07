Map this project into its context layer. Fletch has already recorded one `module` entity per package and per code directory under each package's `src/`, each anchored at its directory and linked `part_of` its parent, but with no summary. Your job is to write down what those parts mean, using codegraph to read the code and the `context_*` ops to record.

1. Call `context_get` with no args to see the map: every module with its slug, path and relations.
2. Read the code. Use `codegraph_explore` per module (its path, its main types and entry points) rather than reading files one by one; skim the README and `docs/` for what the product is for.
3. Record the **vision**: `context_record_entity` with `kind: "vision"`, slug `vision`, a name, and one paragraph of at most 600 characters on what the product is, who it is for and what it is not. If a vision entity already exists, revise it (`id` set to its slug) only when it is empty or plainly wrong.
4. For **every** module, revise it with `context_record_entity` passing `id` (its slug), the same `slug`, `kind: "module"`, its `paths` exactly as `context_get` shows them (the `at` line), and:
   - a one-line `summary` (one sentence, under 200 characters) of what it is responsible for, in the project's own terms;
   - a clearer `name` when the directory name says little (`src` → "Desktop UI");
   - `aliases`: the names people and the code use for it (crate or package name, the concept it implements), so `context_get` finds it by those too.
5. Add the relations that are missing with `context_link`: `depends_on` where one module calls into or imports another in a way that matters to someone changing it, and `part_of` where a module belongs under another the layout did not show. Do not remove the relations that are there.
6. If an important part of the system has no module (a cross-cutting capability, a directory the layout rules missed), record it with `context_record_entity`, `kind: "module"` or `"capability"`, with `paths`, and link it.

Record entities and relations only. Do **not** call `context_record_decision`: decisions, constraints and facts come from the people who made them, not from a reading of the code. Do not edit code, create branches or open a PR. Finish with a short list of what you recorded and anything you were unsure of.
