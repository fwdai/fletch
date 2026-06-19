#!/usr/bin/env bun
// Regenerate the bundled model-catalog snapshot from models.dev.
//
//   bun run scripts/fetch-models-catalog.ts
//
// Fetches the full api.json, slims it via the same transform used at runtime,
// and writes src-tauri/resources/models-catalog.json, the snapshot packaged
// with the app as a Tauri resource (read from disk at runtime, not bundled into
// the JS). Run periodically (or in CI) to refresh the offline baseline; the
// running app self-updates from the network regardless.

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { slimFullCatalog } from "../src/data/modelCatalog/slim";

const URL = "https://models.dev/api.json";
const OUT = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "src-tauri",
  "resources",
  "models-catalog.json",
);

const res = await fetch(URL);
if (!res.ok) {
  console.error(`Failed to fetch ${URL}: ${res.status} ${res.statusText}`);
  process.exit(1);
}
const api = (await res.json()) as Record<string, never>;
const slim = slimFullCatalog(api);
const count = Object.keys(slim).length;
if (count === 0) {
  console.error("Slimmed catalog is empty — aborting without writing.");
  process.exit(1);
}

writeFileSync(OUT, JSON.stringify(slim, null, 2) + "\n");
console.log(`Wrote ${count} models to ${OUT}`);
