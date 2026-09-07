import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import tsconfigPaths from "vite-tsconfig-paths";
import { defineConfig } from "vitest/config";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [tsconfigPaths(), react()],
  resolve: {
    // vite-tsconfig-paths only rewrites imports inside this project, and the
    // reused desktop files (../src/**) are written against the desktop's own
    // `@/*` alias — so map both prefixes here. `partial-json` needs an alias
    // too: Node resolution from ../src/adapters never reaches
    // mobile/node_modules.
    alias: [
      { find: /^@desktop\//, replacement: `${fileURLToPath(new URL("../src/", import.meta.url))}` },
      { find: /^@\//, replacement: `${fileURLToPath(new URL("../src/", import.meta.url))}` },
      {
        find: "partial-json",
        replacement: fileURLToPath(new URL("./node_modules/partial-json", import.meta.url)),
      },
    ],
  },
  clearScreen: false,
  server: {
    port: 1430,
    strictPort: true,
    // Bound to all interfaces when TAURI_DEV_HOST is set so a physical iPhone
    // can load the dev server over the LAN.
    host: host || "0.0.0.0",
    hmr: host ? { protocol: "ws", host, port: 1431 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  test: {
    environment: "jsdom",
    // `?mock` puts the store on the in-memory host; `mockSpeed` collapses the
    // demo-paced scripted stream so the tests don't wait it out.
    environmentOptions: { jsdom: { url: "http://localhost/?mock=1&mockSpeed=0.02" } },
    include: ["tests/**/*.test.ts"],
  },
});
