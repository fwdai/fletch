/**
 * Quorum updater proxy for the private GitHub repo `fwdai/quorum`.
 *
 * The Tauri updater (and download buttons) must fetch artifacts with no
 * credentials, but a private repo's release assets require auth. This Worker
 * holds a fine-grained GitHub PAT (Contents: read) as a Worker secret and
 * proxies the public-facing requests:
 *
 * - `GET /latest.json` — fetches the release's `latest.json` asset, rewrites
 *   each platform's binary URL to point back at this Worker, and serves it so
 *   the updater can download without any GitHub auth of its own.
 * - `GET /download/<filename>` — streams the matching release asset using the PAT.
 * - `GET /download/latest` — 302 to the latest `.dmg` (handy for a website
 *   "Download" button that should always point at the newest build).
 *
 * Only the latest *published, non-draft* release is served (GitHub's
 * /releases/latest excludes drafts), which matches the release flow: CI creates
 * a draft, you publish it, and only then does the updater see it.
 *
 * Required Worker secret: GITHUB_TOKEN (fine-grained PAT, Contents: read on fwdai/quorum).
 */

const REPO_OWNER = "fwdai";
const REPO_NAME = "quorum";

interface Env {
  GITHUB_TOKEN: string;
}

interface ReleaseAsset {
  name: string;
  url: string;
}

interface Release {
  assets: ReleaseAsset[];
}

interface Manifest {
  platforms: Record<string, { url: string; signature: string }>;
  [key: string]: unknown;
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);

    if (url.pathname === "/" || url.pathname === "/latest.json") {
      return getManifest(env, url);
    }

    if (url.pathname === "/download/latest") {
      return redirectToLatestDmg(env, url);
    }

    if (url.pathname.startsWith("/download/")) {
      const filename = decodeURIComponent(url.pathname.slice("/download/".length));
      return downloadAsset(env, filename);
    }

    return new Response("Not found", { status: 404 });
  },
};

async function getManifest(env: Env, requestUrl: URL): Promise<Response> {
  const release = await fetchRelease(env);
  const manifestAsset = release.assets.find((a) => a.name === "latest.json");
  if (!manifestAsset) {
    return new Response("No latest.json in release", { status: 404 });
  }

  const upstream = await fetchAsset(env, manifestAsset.url);
  if (!upstream.ok) {
    return new Response(`Upstream ${upstream.status}`, { status: 502 });
  }

  const manifest = (await upstream.json()) as Manifest;

  // Rewrite each platform's binary URL so the updater hits this Worker, not
  // GitHub. We resolve by filename, so it works whether the manifest carries a
  // browser_download_url or an API asset URL.
  for (const platform of Object.keys(manifest.platforms)) {
    const original = manifest.platforms[platform].url;
    const filename = original.split("/").pop() ?? "";
    manifest.platforms[platform].url = `${requestUrl.origin}/download/${encodeURIComponent(filename)}`;
  }

  return new Response(JSON.stringify(manifest), {
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": "public, max-age=60",
    },
  });
}

async function redirectToLatestDmg(env: Env, requestUrl: URL): Promise<Response> {
  const release = await fetchRelease(env);
  const asset =
    release.assets.find(
      (a) => a.name.endsWith(".dmg") && /universal|aarch64|arm64|silicon/i.test(a.name),
    ) ?? release.assets.find((a) => a.name.endsWith(".dmg"));
  if (!asset) {
    return new Response("No .dmg in latest release", { status: 404 });
  }
  const target = `${requestUrl.origin}/download/${encodeURIComponent(asset.name)}`;
  return Response.redirect(target, 302);
}

async function downloadAsset(env: Env, filename: string): Promise<Response> {
  const release = await fetchRelease(env);
  const asset = release.assets.find((a) => a.name === filename);
  if (!asset) return new Response("Asset not found", { status: 404 });

  const upstream = await fetchAsset(env, asset.url);
  if (!upstream.ok) {
    return new Response(`Upstream ${upstream.status}`, { status: 502 });
  }

  return new Response(upstream.body, {
    status: 200,
    headers: {
      "Content-Type": upstream.headers.get("Content-Type") ?? "application/octet-stream",
      "Content-Length": upstream.headers.get("Content-Length") ?? "",
    },
  });
}

async function fetchRelease(env: Env): Promise<Release> {
  const res = await fetch(
    `https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest`,
    {
      headers: {
        Authorization: `token ${env.GITHUB_TOKEN}`,
        Accept: "application/vnd.github+json",
        "User-Agent": "quorum-updater-proxy",
      },
    },
  );
  if (!res.ok) {
    throw new Error(`GitHub release fetch failed: ${res.status}`);
  }
  return (await res.json()) as Release;
}

async function fetchAsset(env: Env, assetApiUrl: string): Promise<Response> {
  return fetch(assetApiUrl, {
    headers: {
      Authorization: `token ${env.GITHUB_TOKEN}`,
      Accept: "application/octet-stream",
      "User-Agent": "quorum-updater-proxy",
    },
    redirect: "follow",
  });
}
