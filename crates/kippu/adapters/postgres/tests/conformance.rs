//! Runs the `kippu-store` conformance suite against PostgreSQL, each case in a fresh schema.
//!
//! Set `KIPPU_TEST_POSTGRES_URL` (e.g. `postgres://kippu:kippu@localhost:5432/kippu`) to run
//! it; without it every case is skipped.
#![allow(
    clippy::unwrap_used,
    reason = "test setup: failing to set up is a test failure"
)]

use std::str::FromStr;

use kippu_store::Store;
use kippu_store::conformance::Harness;
use kippu_store_postgres::PostgresStore;
use sqlx::postgres::PgConnectOptions;

async fn fresh() -> Option<Harness> {
    let url = std::env::var("KIPPU_TEST_POSTGRES_URL").ok()?;
    let schema = format!("conformance_{}", uuid::Uuid::now_v7().simple());
    let admin = PostgresStore::connect(&url).await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(admin.pool())
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let store = PostgresStore::connect_with(options, 8).await.unwrap();
    store.migrate().await.unwrap();
    Some(Harness::new(store, ()))
}

kippu_store::conformance_tests!(optional fresh());
