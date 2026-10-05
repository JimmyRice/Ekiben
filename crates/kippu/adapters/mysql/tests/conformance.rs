//! Runs the `kippu-store` conformance suite against MySQL, each case in a fresh database.
//!
//! Set `KIPPU_TEST_MYSQL_URL` (e.g. `mysql://root:secret@localhost:3306/kippu`; the user must
//! be allowed to create databases) to run it; without it every case is skipped.
#![allow(
    clippy::unwrap_used,
    reason = "test setup: failing to set up is a test failure"
)]

use std::str::FromStr;

use kippu_store::Store;
use kippu_store::conformance::Harness;
use kippu_store_mysql::MySqlStore;
use sqlx::mysql::MySqlConnectOptions;

async fn fresh() -> Option<Harness> {
    let url = std::env::var("KIPPU_TEST_MYSQL_URL").ok()?;
    let database = format!("conformance_{}", uuid::Uuid::now_v7().simple());
    let admin = MySqlStore::connect(&url).await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {database}")))
        .execute(admin.pool())
        .await
        .unwrap();
    let options = MySqlConnectOptions::from_str(&url)
        .unwrap()
        .database(&database);
    let store = MySqlStore::connect_with(options, 8).await.unwrap();
    store.migrate().await.unwrap();
    Some(Harness::new(store, ()))
}

kippu_store::conformance_tests!(optional fresh());
