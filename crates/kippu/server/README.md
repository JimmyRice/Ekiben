# kippu-server

The launcher that turns [`kippu-core`](../core) into a running service: the `kippu` command
line, configuration sources and storage adapter wiring.

```text
kippu serve [--listen 0.0.0.0:8080] [--no-workers]   HTTP API (+ background workers)
kippu worker                                         background workers only
kippu migrate                                        apply database migrations
kippu keygen [--pem] [--out <prefix>]                new Ed25519 key pair (<prefix>.key/.pub)
kippu root-token --key root.key --name <name>        short-lived root token
kippu verify --key <public key> <ticket>             check a ticket offline, like a gate
kippu config check                                   effective configuration, secrets redacted
```

Logs go to stdout. `--log-format pretty` (the default) writes one block per request, from
arrival to response — method, path, request id, client, caller, idempotency key, the events
logged while handling it, the problem type, status and latency — so concurrent requests never
interleave; background task runs that did something get a block too. `compact` writes one line
per request, and `json` one object per event tagged with `request_id`, plus a
`request finished` object per request. Authorization headers and request bodies are never
logged. `RUST_LOG` filters as usual.

Configuration is merged from, in increasing priority: built-in defaults, the TOML file given
with `--config` (or `KIPPU_CONFIG`), environment variables (`KIPPU_DATABASE__URL`, with `__`
between section and key) and command-line flags. See `kippu.example.toml` at the repository
root.

The `external_login` example is such a distribution: the default modules plus a sign-in module
for a pretend OAuth provider (see `docs/external-login.md`).

Build your own distribution — with extra modules, or different adapters — without forking:

```rust,ignore
fn main() -> std::process::ExitCode {
    kippu_server::Launcher::new()
        .modules(kippu_core::default_modules())
        .module(my_community::Community)
        .run()
}
```
