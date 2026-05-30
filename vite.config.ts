import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { cpSync, existsSync, readdirSync } from "node:fs";
import path from "node:path";

const host = process.env.TAURI_DEV_HOST;

// Copy the Material Icon Theme SVGs into public/ so the File panel can serve
// them as plain static assets (synchronous resolution, no per-icon bundle
// chunks). Runs for both `vite` (dev) and `vite build`. Idempotent.
function copyFileIcons(): Plugin {
  return {
    name: "copy-material-file-icons",
    configResolved() {
      const src = path.resolve(process.cwd(), "node_modules/material-icon-theme/icons");
      const dest = path.resolve(process.cwd(), "public/file-icons");
      if (existsSync(dest) && readdirSync(dest).length > 0) return; // already copied
      if (existsSync(src)) cpSync(src, dest, { recursive: true });
    },
  };
}

export default defineConfig(async () => ({
  plugins: [react(), copyFileIcons()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
}));
