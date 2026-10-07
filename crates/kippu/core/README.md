# kippu-core

Kippu's application core: the module system, authentication and authorization, the HTTP API
and the background workers. It depends only on storage *ports*, never on a concrete adapter.

```text
Kippu::new()
    .modules(kippu_core::default_modules())   // accounts, catalog, admission, purchasing, …
    .module(MyCommunityModule)                 // your own routes, permissions and tasks
    .inbox(nats).event_bus(nats)               // optional: messaging instead of the database queue
    .object_storage(s3)                        // optional: event images
    .build(config, store, clock)?              // → App { router, background tasks }
```

Every built-in feature is a [`Module`] laid out the same way, so a new module is a copy of an
existing one:

| File | Holds | Never |
|---|---|---|
| `mod.rs` | the `Module` impl: permissions, grants, routes, background tasks | logic |
| `service.rs` or `service/` | the use cases: who may do what (`authorize`), validation, store calls, audit, events | `axum`, `dto` |
| `routes.rs` | thin handlers: extractors and headers → one service call → status, body, headers | store calls, `authorize`, business rules |
| `dto.rs` | the API's request and response bodies (`ToSchema`), and `From` conversions to and from the service's types | rules |

A service function takes the [`AppState`], the caller (`&Principal`, or
`Option<&Principal>` for public reads), ids and a plain input struct of its own (`NewEvent`,
`EventChanges`, …), and returns domain types or its own output structs; errors are
[`ApiError`]s (problem kinds are part of the API). Services are public, so your own modules
can reuse them — for example `catalog::service::writable_event` to check that the caller may
edit an event.

- **Changing a rule** (who may publish, a new limit, another event on the outbox): edit the
  service; routes and API types stay as they are.
- **Changing the API** (a field name, a new header, another status code): edit `dto.rs` and
  `routes.rs`; the service stays as it is.
- **Adding a feature**: write the use case in `service`, then a handler in `routes.rs`, its
  bodies in `dto.rs`, and register the handler in `mod.rs`. In a separate crate, the same
  files make up your own `Module`.

Every listing answers [`http::Listing`] — `{"items": [...], "next_cursor": ...}` — and pages
with opaque keyset cursors. A paged service takes a `PageRequest<P>`, reads
`page.plus_one()` from the store and returns `Page::from_lookahead(records, page.limit, …)`;
its handler takes [`http::PageQuery`], calls `query.page(tag)?` and answers
`Listing::page(page, tag, convert)`. The tag names the listing inside its cursors so one
cannot resume another (a fork's modules use [`http::ListingTag`] values from 128 up). Lists
that cannot grow answer `Listing::all(items)`, so clients read every listing the same way.
