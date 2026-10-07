# kippu-objects-s3

Amazon S3 and S3-compatible services (MinIO, Cloudflare R2, Alibaba OSS, `SeaweedFS`, …) as Kippu's object storage, for event images. Selected by an `images.url` with the
scheme `s3://` and `s3a://`:

```toml
[images]
url = "s3://kippu-images/production"
```

The URL's path is a key prefix, so deployments can share a bucket. Credentials come from the
provider's usual environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, `AWS_ENDPOINT` (for compatible services)) or from `[images.options]`, whose names are
those variables in lower case (`aws_endpoint`, …); unknown names are rejected.

Enable it in the `kippu` binary with the `s3` feature (on by default). This crate is a thin
layer over [`kippu-objects-common`](https://docs.rs/kippu-objects-common).
