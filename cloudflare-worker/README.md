# Quorum updater proxy (Cloudflare Worker)

Because `fwdai/quorum` is a **private** repo, its release assets can't be fetched
without authentication — but the Tauri auto-updater and download links must work
with no credentials. This Worker holds a GitHub PAT as a secret and proxies the
public requests. See `src/index.ts` for the route details.

It serves only the latest **published, non-draft** release.

## Deploy

```sh
cd cloudflare-worker
bun install            # or npm install

# One-time: store the GitHub token as a Worker secret (NOT in wrangler.toml).
# Use a fine-grained PAT scoped to fwdai/quorum with "Contents: read".
wrangler secret put GITHUB_TOKEN

wrangler deploy
```

## Custom domain

`wrangler.toml` binds the Worker to `updater.quorum.fwdai.org`, which is the URL in
`src-tauri/tauri.conf.json` → `plugins.updater.endpoints`. The domain must be on
your Cloudflare account; Wrangler provisions the custom-domain route on deploy.
If you use a different host, update both `wrangler.toml` and `tauri.conf.json`.

## Endpoints

- `GET /latest.json` — updater manifest with binary URLs rewritten to this Worker.
- `GET /download/<filename>` — streams a named asset from the latest release.
- `GET /download/latest` — 302 to the latest universal `.dmg`.
