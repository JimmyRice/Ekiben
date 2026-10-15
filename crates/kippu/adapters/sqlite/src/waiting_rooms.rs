//! Waiting rooms. The other adapters hand out positions from many counter rows per room so
//! that joins running together do not wait for each other; SQLite has one writer, so joins
//! never run together and a room has a single counter (stride 1). Positions are handed out one
//! after another with none missing, and admission never has any to skip.

use kippu_domain::admission::WaitingRoom;
use kippu_domain::{AccountId, SaleId, Timestamp};
use kippu_store::{StoreError, StoreResult};
use sqlx::{SqliteConnection, SqlitePool};
use uuid::Uuid;

use crate::convert::micros;
use crate::error;

/// Makes a sale's waiting room and its counter, unless they exist.
pub(crate) async fn create(connection: &mut SqliteConnection, sale: SaleId) -> StoreResult<()> {
    sqlx::query(
        "INSERT INTO waiting_rooms (sale_id, admitted_through) VALUES (?1, 0)
         ON CONFLICT (sale_id) DO NOTHING",
    )
    .bind(sale.as_uuid())
    .execute(&mut *connection)
    .await
    .map_err(error)?;
    sqlx::query(
        "INSERT INTO waiting_room_counters (sale_id, counter, next_position, stride)
         VALUES (?1, 0, 1, 1) ON CONFLICT (sale_id, counter) DO NOTHING",
    )
    .bind(sale.as_uuid())
    .execute(&mut *connection)
    .await
    .map_err(error)?;
    Ok(())
}

pub(crate) async fn join(
    reader: &SqlitePool,
    writer: &SqlitePool,
    sale: SaleId,
    account: AccountId,
) -> StoreResult<u64> {
    if let Some(position) = entry(reader, sale, account).await? {
        return Ok(position);
    }
    // A sale that took a waiting room after it was created gets it here.
    for make_room in [false, true] {
        if make_room {
            let mut tx = writer.begin_with("BEGIN IMMEDIATE").await.map_err(error)?;
            create(&mut tx, sale).await?;
            tx.commit().await.map_err(error)?;
        }
        let taken = sqlx::query_scalar::<_, i64>(
            "UPDATE waiting_room_counters SET next_position = next_position + stride
             WHERE sale_id = ?1 AND counter = (SELECT counter FROM waiting_room_counters
                                               WHERE sale_id = ?1 ORDER BY next_position LIMIT 1)
             RETURNING next_position - stride",
        )
        .bind(sale.as_uuid())
        .fetch_optional(writer)
        .await
        .map_err(error)?;
        let Some(position) = taken else {
            continue;
        };
        // The same account joining at the same moment may have entered first: its position
        // stands, and this one is left unused.
        let entered = sqlx::query(
            "INSERT INTO waiting_room_entries (sale_id, account_id, position) VALUES (?1, ?2, ?3)
             ON CONFLICT (sale_id, account_id) DO NOTHING",
        )
        .bind(sale.as_uuid())
        .bind(account.as_uuid())
        .bind(position)
        .execute(writer)
        .await
        .map_err(error)?
        .rows_affected()
            == 1;
        if entered {
            return u64::try_from(position).map_err(StoreError::backend);
        }
        return entry(writer, sale, account)
            .await?
            .ok_or_else(|| StoreError::backend("a waiting room entry vanished"));
    }
    Err(StoreError::backend("a waiting room has no counter"))
}

pub(crate) async fn admitted_through(reader: &SqlitePool, sale: SaleId) -> StoreResult<u64> {
    let admitted = sqlx::query_scalar::<_, i64>(
        "SELECT admitted_through FROM waiting_rooms WHERE sale_id = ?1",
    )
    .bind(sale.as_uuid())
    .fetch_optional(reader)
    .await
    .map_err(error)?;
    u64::try_from(admitted.unwrap_or(0)).map_err(StoreError::backend)
}

pub(crate) async fn room(reader: &SqlitePool, sale: SaleId) -> StoreResult<WaitingRoom> {
    let row = sqlx::query_as::<_, (i64, Option<i64>)>(
        "SELECT admitted_through,
                (SELECT MAX(next_position - stride) FROM waiting_room_counters WHERE sale_id = ?1)
         FROM waiting_rooms WHERE sale_id = ?1",
    )
    .bind(sale.as_uuid())
    .fetch_optional(reader)
    .await
    .map_err(error)?;
    let Some((admitted_through, last_position)) = row else {
        return Ok(WaitingRoom::default());
    };
    Ok(WaitingRoom {
        last_position: u64::try_from(last_position.unwrap_or(0)).map_err(StoreError::backend)?,
        admitted_through: u64::try_from(admitted_through).map_err(StoreError::backend)?,
    })
}

pub(crate) async fn pending(reader: &SqlitePool, now: Timestamp) -> StoreResult<Vec<SaleId>> {
    // Only open sales' rooms have their counters read: rooms of past sales add up, and this
    // runs on every admission tick of every instance.
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT r.sale_id FROM waiting_rooms r JOIN sales s ON s.id = r.sale_id
         WHERE s.closes_at > ?1
           AND r.admitted_through < (SELECT MAX(next_position - stride)
                                     FROM waiting_room_counters c WHERE c.sale_id = r.sale_id)
         ORDER BY r.sale_id",
    )
    .bind(micros(now))
    .fetch_all(reader)
    .await
    .map_err(error)?;
    Ok(ids.into_iter().map(Into::into).collect())
}

pub(crate) async fn advance(
    writer: &SqlitePool,
    sale: SaleId,
    from: u64,
    to: u64,
) -> StoreResult<Option<u64>> {
    let (from, to) = (
        i64::try_from(from).map_err(StoreError::backend)?,
        i64::try_from(to).map_err(StoreError::backend)?,
    );
    // Every position below the counter's next one has been handed out: admission goes up to
    // there, never past it.
    let through = sqlx::query_scalar::<_, i64>(
        "UPDATE waiting_rooms
         SET admitted_through = MIN(?3, (SELECT MIN(next_position) - 1 FROM waiting_room_counters
                                         WHERE sale_id = ?1))
         WHERE sale_id = ?1 AND admitted_through = ?2
           AND (SELECT MIN(next_position) - 1 FROM waiting_room_counters WHERE sale_id = ?1) > ?2
         RETURNING admitted_through",
    )
    .bind(sale.as_uuid())
    .bind(from)
    .bind(to)
    .fetch_optional(writer)
    .await
    .map_err(error)?;
    through
        .map(|through| u64::try_from(through).map_err(StoreError::backend))
        .transpose()
}

/// The position `account` holds in a sale's waiting room, if it joined.
async fn entry(pool: &SqlitePool, sale: SaleId, account: AccountId) -> StoreResult<Option<u64>> {
    let position = sqlx::query_scalar::<_, i64>(
        "SELECT position FROM waiting_room_entries WHERE sale_id = ?1 AND account_id = ?2",
    )
    .bind(sale.as_uuid())
    .bind(account.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(error)?;
    position
        .map(|position| u64::try_from(position).map_err(StoreError::backend))
        .transpose()
}
