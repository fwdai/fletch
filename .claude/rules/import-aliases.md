---
paths:
  - "src/**/*.{ts,tsx}"
---

# Import path aliases

Use the `@/` alias for imports from `src/`. It maps to the project root `src/` directory (see `tsconfig.json` and `vite.config.ts`).

## Do

```typescript
import { useAppStore } from "@/store";
import { Composer } from "@/components/Composer";
import { applyPolicy } from "@/adapters";
```

Same-folder imports may stay relative:

```typescript
import { ChatNav } from "./ChatNav";
import { pairToolItems } from "./messages/pair";
```

## Don't

```typescript
import { useAppStore } from "../../store";
import { Composer } from "../Composer";
```

## Exceptions

- `./` imports within the same module or folder are fine.
- Imports outside `src/` (e.g. `../../../package.json`) stay relative — there is no alias for those.

Biome enforces this via `style/noRestrictedImports` (`../**` is an error).
