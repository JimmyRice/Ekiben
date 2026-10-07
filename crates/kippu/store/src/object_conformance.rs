//! The conformance suite of the object storage port.
//!
//! Every object storage adapter runs it against itself with one line:
//!
//! ```ignore
//! kippu_store::object_conformance_tests!(async {
//!     let objects = MyObjects::connect(...).unwrap();
//!     Some(std::sync::Arc::new(objects) as std::sync::Arc<dyn kippu_store::ObjectStorage>)
//! });
//! ```
//!
//! The expression evaluates to a future of an `Option`: `None` skips the cases, for adapters
//! that need a cloud account the environment may not provide. Each case uses keys of its own,
//! so a bucket can be shared between runs.
#![allow(
    clippy::unwrap_used,
    clippy::missing_panics_doc,
    missing_docs,
    reason = "test code: a failed expectation is the test failing"
)]

use bytes::{Bytes, BytesMut};
use futures_util::TryStreamExt;

use crate::{ObjectError, ObjectStorage};

/// Expands to one `#[tokio::test]` per conformance case.
#[macro_export]
macro_rules! object_conformance_tests {
    ($connect:expr) => {
        $crate::object_conformance_tests!(@cases $connect;
            stored_objects_are_read_back_whole,
            putting_again_replaces,
            absent_objects_are_not_found,
            deleting_is_idempotent,
            keys_do_not_collide,
        );
    };
    (@cases $connect:expr; $($case:ident),* $(,)?) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn $case() {
                let connected: Option<std::sync::Arc<dyn $crate::ObjectStorage>> = $connect.await;
                let Some(objects) = connected else {
                    eprintln!("skipped: no object storage configured for this adapter");
                    return;
                };
                $crate::object_conformance::$case(objects.as_ref()).await;
            }
        )*
    };
}

fn key(name: &str) -> String {
    format!("conformance/{}/{name}", uuid::Uuid::now_v7().simple())
}

async fn put(objects: &dyn ObjectStorage, key: &str, bytes: &[u8]) {
    objects
        .put(
            key,
            Bytes::copy_from_slice(bytes),
            "image/png",
            "public, max-age=60",
        )
        .await
        .unwrap();
}

async fn read(objects: &dyn ObjectStorage, key: &str) -> Result<Bytes, ObjectError> {
    let object = objects.get(key).await?;
    let size = object.size;
    let chunks: Vec<Bytes> = object
        .body
        .try_collect()
        .await
        .map_err(ObjectError::failed)?;
    let mut whole = BytesMut::new();
    for chunk in chunks {
        whole.extend_from_slice(&chunk);
    }
    assert_eq!(whole.len() as u64, size, "the size matches the bytes");
    Ok(whole.freeze())
}

pub async fn stored_objects_are_read_back_whole(objects: &dyn ObjectStorage) {
    // Larger than any single chunk a service is likely to send.
    let payload: Vec<u8> = (0..1_500_000_u32).map(|n| (n % 251) as u8).collect();
    let key = key("whole.png");
    put(objects, &key, &payload).await;
    assert_eq!(read(objects, &key).await.unwrap().as_ref(), payload);
}

pub async fn putting_again_replaces(objects: &dyn ObjectStorage) {
    let key = key("replaced.png");
    put(objects, &key, b"first").await;
    put(objects, &key, b"second, longer").await;
    assert_eq!(
        read(objects, &key).await.unwrap().as_ref(),
        b"second, longer"
    );
}

pub async fn absent_objects_are_not_found(objects: &dyn ObjectStorage) {
    let error = objects.get(&key("never-stored.png")).await.unwrap_err();
    assert!(matches!(error, ObjectError::NotFound), "{error:?}");
}

pub async fn deleting_is_idempotent(objects: &dyn ObjectStorage) {
    let key = key("deleted.png");
    put(objects, &key, b"bytes").await;
    objects.delete(&key).await.unwrap();
    assert!(matches!(
        objects.get(&key).await.unwrap_err(),
        ObjectError::NotFound
    ));
    objects.delete(&key).await.unwrap();
    objects
        .delete(&self::key("never-stored.png"))
        .await
        .unwrap();
}

pub async fn keys_do_not_collide(objects: &dyn ObjectStorage) {
    let (a, b) = (key("dir/a.png"), key("dir/b.png"));
    put(objects, &a, b"a").await;
    put(objects, &b, b"b").await;
    objects.delete(&a).await.unwrap();
    assert!(matches!(
        objects.get(&a).await.unwrap_err(),
        ObjectError::NotFound
    ));
    assert_eq!(read(objects, &b).await.unwrap().as_ref(), b"b");
}
