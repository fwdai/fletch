// `cloudflare:test` and `cloudflare:workers` type their `env` as
// `Cloudflare.Env`, which `wrangler types` would normally generate into a
// megabyte of bindings. The relay has one binding and four optional secrets,
// so they are declared by hand here instead.
declare namespace Cloudflare {
  interface Env {
    HOSTS: DurableObjectNamespace;
    // `wrangler secret put APNS_…`; a relay without them ignores NOTIFY.
    APNS_TEAM_ID?: string;
    APNS_KEY_ID?: string;
    APNS_PRIVATE_KEY?: string;
    APNS_BUNDLE_ID?: string;
  }
}
