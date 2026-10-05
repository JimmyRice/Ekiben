//! Runs the `kippu-store` conformance suite against SQLite, each case on a fresh database file.
#![allow(clippy::unwrap_used, reason = "test setup: failing to set up is a test failure")]

use kippu_store::Store;
use kippu_store::conformance::Harness;
use kippu_store_sqlite::SqliteStore;

async fn fresh() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("kippu.db").display());
    let store = SqliteStore::connect(&url).await.unwrap();
    store.migrate().await.unwrap();
    Harness::new(store, dir)
}

kippu_store::conformance_tests!(fresh());
