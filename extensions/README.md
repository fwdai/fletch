# Extensions

Self-contained units that extend the app through the typed seams in
[`src/extensions/types.ts`](../src/extensions/types.ts). The core knows the
*shape* of an extension, never which ones exist.

## Discovery

Every `extensions/<name>/index.ts` present at build time is loaded automatically
([`src/extensions/registry.ts`](../src/extensions/registry.ts)). There is no
manifest and no enable flag — an extension is active iff its folder is on disk
when the app is built. Drop in a folder, rebuild, done.

## Anatomy

Every part is optional — an extension uses only what it needs. The folder name
is the extension id.

```
extensions/my-extension/
  index.ts            # frontend contributions (optional)
  *.tsx               # components
  migrations/*.sql    # DB tables it owns (optional)
  backend/mod.rs      # custom Rust commands (optional)
```

### Frontend (`index.ts`)

```ts
import type { Extension } from "../../src/extensions/types";
import { MyPane } from "./SettingsPane";

export const extension: Extension = {
  id: "my-extension",
  settingsPanes: [
    { id: "ext:my-extension", label: "My Extension", icon: "zap", Component: MyPane },
  ],
};
```

### Database (`migrations/*.sql`)

Drop SQL files in `migrations/`; they run once, in filename order, the first
time a build that has them starts. Tracked in `_ext_migrations`, separate from
the core schema — so installing/removing an extension never disturbs the core.

```sql
-- migrations/0001_init.sql
CREATE TABLE IF NOT EXISTS my_table (id INTEGER PRIMARY KEY, body TEXT NOT NULL);
```

For plain CRUD you need **no Rust** — the frontend can use the core's generic
`db_insert` / `db_select` / `db_query` / … commands (see `src/api.ts`) against
your own tables.

### Backend (`backend/mod.rs`)

Only when you need logic beyond CRUD. Register handlers; they're reached from
the frontend via `callExtension(id, command, args)`. No Tauri command list edits
and no capability/ACL config — the core routes everything through one
`ext_invoke` command.

```rust
use crate::extensions::prelude::*;

pub fn register(api: &mut Registrar) {
    api.command("count_notes", |_args, ctx| {
        let count: i64 = ctx.db
            .query_row("SELECT COUNT(*) FROM my_table", (), |r| r.get(0))
            .map_err(|e| e.to_string())?;
        Ok(json!({ "count": count }))
    });
}
```

```ts
import { callExtension } from "../../src/extensions/backend";
const { count } = await callExtension<{ count: number }>("my-extension", "count_notes");
```

Backend code is compiled into the app **only when the extension folder is
present**, so private/unpublished extensions never reach the public build.

## Public vs private

The two are loaded identically; the only difference is where the source lives:

- **Public / community** — committed here under the repo's license, shipped in
  the official build.
- **Private** — its own repo, cloned into a `*.local/` folder (e.g.
  `extensions/sync.local/`), which `.gitignore` keeps out of this repo. A fresh
  public clone / CI doesn't have it, so it never reaches the public build. Why a
  given extension is private — unreleased, in development, separately
  licensed — is an implementation detail the core does not model.
