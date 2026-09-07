// `cloudflare:test` and `cloudflare:workers` type their `env` as
// `Cloudflare.Env`, which `wrangler types` would normally generate into a
// megabyte of bindings. The relay has exactly one binding, so it is declared
// by hand here instead.
declare namespace Cloudflare {
  interface Env {
    HOSTS: DurableObjectNamespace;
  }
}
