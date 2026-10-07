//! Runs the `kippu-store` conformance suite against MySQL, each case in a fresh database.
//!
//! Set `EKIBEN_TEST_MYSQL_URL` (e.g. `mysql://root:secret@localhost:3306/kippu`; the user must
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

/// Drops a test database when the case ends, so a run does not fill the server with tables
/// (MySQL then starts failing prepared statements with error 1615).
struct DropDatabase {
    url: String,
    name: String,
}

impl Drop for DropDatabase {
    fn drop(&mut self) {
        let (url, name) = (self.url.clone(), self.name.clone());
        // Drop runs inside the test's runtime, which must not be blocked: use a thread.
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let Ok(admin) = MySqlStore::connect(&url).await else {
                    return;
                };
                // Connections of the test may still hold an unfinished transaction (sqlx
                // rolls back lazily), whose metadata locks would block the drop forever.
                let holders = sqlx::query_scalar::<_, u64>(
                    "SELECT id FROM information_schema.processlist WHERE db = ?",
                )
                .bind(&name)
                .fetch_all(admin.pool())
                .await
                .unwrap_or_default();
                for id in holders {
                    let kill = format!("KILL {id}");
                    let _ = sqlx::query(sqlx::AssertSqlSafe(kill))
                        .execute(admin.pool())
                        .await;
                }
                let mut connection = admin.pool().acquire().await.unwrap();
                let _ = sqlx::query("SET SESSION lock_wait_timeout = 10")
                    .execute(&mut *connection)
                    .await;
                let drop = format!("DROP DATABASE IF EXISTS {name}");
                let _ = sqlx::query(sqlx::AssertSqlSafe(drop))
                    .execute(&mut *connection)
                    .await;
            });
        })
        .join();
    }
}

async fn fresh() -> Option<Harness> {
    let url = std::env::var("EKIBEN_TEST_MYSQL_URL").ok()?;
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
    Some(Harness::new(
        store,
        DropDatabase {
            url,
            name: database,
        },
    ))
}

kippu_store::conformance_tests!(optional fresh());
