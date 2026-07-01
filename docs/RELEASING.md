# Releasing Fletch

Fletch ships as a signed + notarized **universal** macOS app, distributed via
GitHub Releases, with a silent in-app auto-updater. This doc covers the one-time
setup and the per-release procedure.

## Overview

```
bump version → run Release workflow (manual) → CI builds/signs/notarizes
            → DRAFT GitHub release (.dmg + .app.tar.gz + .sig + latest.json)
            → you publish the draft → endpoint serves latest.json → apps update
```

- **Build:** `universal-apple-darwin` (single binary for Apple Silicon + Intel).
- **Sign:** Developer ID Application cert (Team `UFBL3F444A`).
- **Notarize + staple:** handled automatically by `tauri-apps/tauri-action`.
- **Update artifacts:** `createUpdaterArtifacts: true` emits `<app>.app.tar.gz`
  and a `.sig` signed with the Fletch minisign key.

## One-time setup

### 1. Updater signing keypair

A minisign keypair signs every update artifact; the app verifies it with the
public key embedded in `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`).

A keypair was generated for Fletch and the **public** key is already committed.
The **private** key lives outside the repo at `~/.tauri/quorum.key`. To
regenerate (e.g. to set a password), run:

```sh
bun run tauri signer generate -w ~/.tauri/quorum.key
```

Then update `plugins.updater.pubkey` in `tauri.conf.json` with the new
`~/.tauri/quorum.key.pub` contents. **If you lose the private key, existing
installs can no longer auto-update** — they'd need a fresh manual install.

### 2. GitHub repository secrets

Set these under **Settings → Secrets and variables → Actions**:

| Secret | How to produce it |
| --- | --- |
| `APPLE_CERTIFICATE` | Export your "Developer ID Application" cert + key from Keychain as a `.p12`, then `base64 -i cert.p12 \| pbcopy`. |
| `APPLE_CERTIFICATE_PASSWORD` | The password you set when exporting the `.p12`. |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Oleksandr Chaplinsky (UFBL3F444A)` (from `security find-identity -v -p codesigning`). |
| `APPLE_ID` | Apple ID email of the developer account. |
| `APPLE_PASSWORD` | An **app-specific password** (appleid.apple.com → Sign-In and Security → App-Specific Passwords). Not your account password. |
| `APPLE_TEAM_ID` | `UFBL3F444A`. |
| `TAURI_SIGNING_PRIVATE_KEY` | Contents of `~/.tauri/quorum.key`. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for that key (empty if generated without one). |
| `QUORUM_POSTHOG_KEY` | PostHog **project** key (write-only, safe to ship), baked in at compile time for anonymous usage telemetry. Optional — unset → telemetry is a no-op. Self-hosting only: also set `QUORUM_POSTHOG_HOST` (defaults to PostHog US cloud). |

### 3. Update endpoint — Cloudflare Worker

Because `fwdai/quorum` is **private**, its release assets need authentication,
but the updater must fetch them without credentials. A Cloudflare Worker in
[`cloudflare-worker/`](../cloudflare-worker/) bridges this: it holds a GitHub PAT
as a secret and proxies `latest.json` + asset downloads, rewriting the manifest
URLs to point back at itself. `tauri.conf.json` already points at it:

```json
"endpoints": ["https://fletch.sh/releases/latest.json"]
```

Deploy it once:

```sh
cd cloudflare-worker
bun install
wrangler secret put GITHUB_TOKEN   # fine-grained PAT, Contents: read on fwdai/quorum
wrangler deploy
```

The `fletch.sh` domain must be on your Cloudflare account, with the Worker
routed on `fletch.sh/releases/*` (Wrangler provisions the route). If you use a
different host or path, change both `cloudflare-worker/wrangler.toml` and
`tauri.conf.json`. See
`cloudflare-worker/README.md` for endpoint details.

The Worker serves only the latest **published, non-draft** release (GitHub's
`/releases/latest` excludes drafts), so the updater only sees a release once you
publish it (see below). Integrity is still enforced by the minisign signature
the app verifies against the embedded pubkey — the proxy can't forge an update.

## Per-release procedure

1. **Bump the version** in all three files (keep them identical):
   - `src-tauri/tauri.conf.json` → `version` (this is what the release tag/name
     is derived from, via `v__VERSION__`)
   - `package.json` → `version`
   - `src-tauri/Cargo.toml` → `version`
2. Merge that to `main`.
3. Run the **Release** workflow: GitHub → Actions → *Release* → *Run workflow*.
4. Wait for it to finish. It creates a **draft** release `Fletch v<version>`
   with the `.dmg`, `.app.tar.gz`, `.app.tar.gz.sig`, and `latest.json`.
5. Review the draft, then **Publish** it. Once published, the endpoint serves
   the new `latest.json` and installed apps pick it up on next launch.

## Local signed build

To produce a signed + notarized universal build on your Mac (no CI):

```sh
cp .env.example .env   # then fill in the values
./scripts/release-local.sh
```

Artifacts land in
`src-tauri/target/universal-apple-darwin/release/bundle/`.

## How the in-app updater behaves

`src/util/autoUpdate.ts` runs once at launch (`src/main.tsx`):

- No-op in dev builds.
- Calls `check()`; if an update is available, `downloadAndInstall()` then
  `relaunch()`.
- Fully silent, no UI. Any failure (offline, endpoint down) is logged and
  swallowed so it never blocks startup.

Capabilities required (`src-tauri/capabilities/default.json`):
`updater:default`, `process:allow-restart`.
