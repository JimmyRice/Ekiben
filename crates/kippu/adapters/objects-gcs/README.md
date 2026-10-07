# kippu-objects-gcs

Google Cloud Storage as Kippu's object storage, for event images. Selected by an `images.url`
with the scheme `gs://`:

```toml
[images]
url = "gs://kippu-images/production"
```

The URL's path is a key prefix, so deployments can share a bucket. Credentials come from the
provider's usual environment variables (`GOOGLE_SERVICE_ACCOUNT`,
`GOOGLE_APPLICATION_CREDENTIALS`) or from `[images.options]`, whose names are
those variables in lower case (`google_service_account`, …); unknown names are rejected.

Enable it in the `kippu` binary with the `gcs` feature (on by default). This crate is a thin
layer over [`kippu-objects-common`](https://docs.rs/kippu-objects-common).
