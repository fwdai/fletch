import { cloudflareTest } from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";

// Tests run inside the same workerd that serves the Worker, so the Durable
// Object, the WebSocket Hibernation API and WebCrypto behave exactly as they
// do in production. `cloudflareTest` is the vitest 4 / pool-workers 0.22
// plugin form of what used to be `defineWorkersConfig`.
export default defineConfig({
  plugins: [cloudflareTest({ wrangler: { configPath: "./wrangler.toml" } })],
});
