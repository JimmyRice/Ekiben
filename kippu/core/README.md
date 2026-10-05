# kippu-core

Kippu's application core: the module system, authentication and authorization, the HTTP API
and the background workers. It depends only on storage *ports*, never on a concrete adapter.

```text
Kippu::new()
    .modules(kippu_core::default_modules())   // accounts, catalog, admission, purchasing, …
    .module(MyCommunityModule)                 // your own routes, permissions and tasks
    .build(config, store, clock)?              // → App { router, background tasks }
```

Every built-in feature is a [`Module`] laid out the same way — `mod.rs` (permissions and
wiring), `routes.rs` (thin handlers with OpenAPI annotations), `service.rs` (use cases),
`dto.rs` (request and response bodies) — so a new module is a copy of an existing one.
