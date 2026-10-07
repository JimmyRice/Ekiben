# kippu-objects-azure

Azure Blob Storage as Kippu's object storage, for event images. Selected by an `images.url` with the
scheme `az://`, `azure://`, `abfs://` and `abfss://`:

```toml
[images]
url = "az://kippu-images/production"
```

The URL's path is a key prefix, so deployments can share a bucket. Credentials come from the
provider's usual environment variables (`AZURE_STORAGE_ACCOUNT_NAME`, `AZURE_STORAGE_ACCOUNT_KEY`) or from `[images.options]`, whose names are
those variables in lower case (`azure_storage_account_name`, …); unknown names are rejected.

Enable it in the `kippu` binary with the `azure` feature (on by default). This crate is a thin
layer over [`kippu-objects-common`](https://docs.rs/kippu-objects-common).
