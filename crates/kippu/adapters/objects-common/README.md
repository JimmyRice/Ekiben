# kippu-objects-common

What Kippu's object storage adapters (`kippu-objects-s3`, `-gcs`, `-azure`) have in common.
They all build on the [`object_store`](https://docs.rs/object_store) crate, so the translation
to Kippu's [`ObjectStorage`](kippu_store::ObjectStorage) port lives here once:

- [`ObjectStoreAdapter`] implements the port for any `object_store::ObjectStore`;
- [`connect`] parses an `images.url`, applies the provider's `images.options` and scopes the
  store to the URL's path prefix.

Writing an adapter for another provider that `object_store` supports is a few lines: build the
store and hand it to [`connect`]. For a service it does not support, implement
[`ObjectStorage`](kippu_store::ObjectStorage) directly (three methods) and pass it to
`Kippu::object_storage`.
