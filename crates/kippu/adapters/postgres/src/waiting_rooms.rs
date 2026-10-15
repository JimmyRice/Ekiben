//! Waiting rooms. A room hands out positions from [`COUNTERS`] counter rows: counter `c` hands
//! out `c + 1`, `c + 1 + COUNTERS`, … (a room from before the counters continues after its
//! last position), and a join takes the lowest position of a counter no other join holds at
//! that moment (`FOR UPDATE SKIP LOCKED`). Joins running together seldom wait for each other,
//! and positions stay all but consecutive: only those being handed out at the same moment can
//! come out of order.

use kippu_domain::admission::WaitingRoom;
use kippu_domain::{AccountId, SaleId, Timestamp};
use kippu_store::{StoreError, StoreResult};
use sqlx::PgPool;
use uuid::Uuid;

use crate::convert::at;
use crate::error;

/// Counter rows per waiting room. Joins wait for each other only when more of them run at
/// once, across all instances, than there are counters.
pub(crate) const COUNTERS: i32 = 128;

/// Makes a sale's waiting room and its counters, unless they exist. Binds the sale and
/// [`COUNTERS`].
const CREATE: &str = "WITH room AS (
         INSERT INTO waiting_rooms (sale_id, admitted_through) VALUES ($1, 0)
         ON CONFLICT (sale_id) DO NOTHING
     )
     INSERT INTO waiting_room_counters (sale_id, counter, next_position, stride)
     SELECT $1, counter, counter + 1, $2 FROM generate_series(0, $2 - 1) AS counter
     ON CONFLICT (sale_id, counter) DO NOTHING";

/// Takes the lowest position of a counter it can lock and enters the account with it. The
/// counter is picked in a CTE joined by `FROM`: as a subquery in `WHERE`, PostgreSQL would run
/// the pick again when it re-checks a counter another join has just moved, and often update
/// nothing. `wait` waits for the lowest counter instead of skipping those other joins hold.
fn enter(wait: bool) -> sqlx::AssertSqlSafe<String> {
    let lock = if wait {
        "FOR UPDATE"
    } else {
        "FOR UPDATE SKIP LOCKED"
    };
    sqlx::AssertSqlSafe(format!(
        "WITH pick AS (
             SELECT counter FROM waiting_room_counters WHERE sale_id = $1
             ORDER BY next_position LIMIT 1 {lock}
         ), taken AS (
             UPDATE waiting_room_counters c SET next_position = c.next_position + c.stride
             FROM pick WHERE c.sale_id = $1 AND c.counter = pick.counter
             RETURNING c.next_position - c.stride AS position
         )
         INSERT INTO waiting_room_entries (sale_id, account_id, position)
         SELECT $1, $2, position FROM taken
         ON CONFLICT (sale_id, account_id) DO NOTHING
         RETURNING position"
    ))
}

/// Makes a sale's waiting room, for a sale that has none yet.
pub(crate) async fn create(pool: &PgPool, sale: SaleId) -> StoreResult<()> {
    sqlx::query(CREATE)
        .bind(sale.as_uuid())
        .bind(COUNTERS)
        .execute(pool)
        .await
        .map_err(error)?;
    Ok(())
}

pub(crate) async fn join(pool: &PgPool, sale: SaleId, account: AccountId) -> StoreResult<u64> {
    if let Some(position) = entry(pool, sale, account).await? {
        return Ok(position);
    }
    // Usually a counter no other join holds. Failing that, every counter is held at this
    // moment, or the sale's room was never made (the sale took a waiting room after it was
    // created): then wait for the lowest counter, after making the room if need be.
    for (wait, make_room) in [(false, false), (true, false), (true, true)] {
        if make_room {
            create(pool, sale).await?;
        }
        let entered = sqlx::query_scalar::<_, i64>(enter(wait))
            .bind(sale.as_uuid())
            .bind(account.as_uuid())
            .fetch_optional(pool)
            .await
            .map_err(error)?;
        if let Some(position) = entered {
            return u64::try_from(position).map_err(StoreError::backend);
        }
        // The same account joining at the same moment entered first: its position stands,
        // and the one taken here, if any, is left unused.
        if let Some(position) = entry(pool, sale, account).await? {
            return Ok(position);
        }
    }
    Err(StoreError::backend("a waiting room has no counters"))
}

pub(crate) async fn admitted_through(pool: &PgPool, sale: SaleId) -> StoreResult<u64> {
    let admitted = sqlx::query_scalar::<_, i64>(
        "SELECT admitted_through FROM waiting_rooms WHERE sale_id = $1",
    )
    .bind(sale.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(error)?;
    u64::try_from(admitted.unwrap_or(0)).map_err(StoreError::backend)
}

pub(crate) async fn room(pool: &PgPool, sale: SaleId) -> StoreResult<WaitingRoom> {
    let row = sqlx::query_as::<_, (i64, Option<i64>)>(
        "SELECT admitted_through,
                (SELECT MAX(next_position - stride) FROM waiting_room_counters WHERE sale_id = $1)
         FROM waiting_rooms WHERE sale_id = $1",
    )
    .bind(sale.as_uuid())
    .fetch_optional(pool)
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

pub(crate) async fn pending(pool: &PgPool, now: Timestamp) -> StoreResult<Vec<SaleId>> {
    // Only open sales' rooms have their counters read: rooms of past sales add up, and this
    // runs on every admission tick of every instance.
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT r.sale_id FROM waiting_rooms r JOIN sales s ON s.id = r.sale_id
         WHERE s.closes_at > $1
           AND r.admitted_through < (SELECT MAX(next_position - stride)
                                     FROM waiting_room_counters c WHERE c.sale_id = r.sale_id)
         ORDER BY r.sale_id",
    )
    .bind(at(now))
    .fetch_all(pool)
    .await
    .map_err(error)?;
    Ok(ids.into_iter().map(Into::into).collect())
}

pub(crate) async fn advance(
    pool: &PgPool,
    sale: SaleId,
    from: u64,
    to: u64,
) -> StoreResult<Option<u64>> {
    let (from, to) = (
        i64::try_from(from).map_err(StoreError::backend)?,
        i64::try_from(to).map_err(StoreError::backend)?,
    );
    let mut tx = pool.begin().await.map_err(error)?;
    // The room's row is the compare-and-set: a ticker on another instance waits here, then
    // finds it moved.
    let admitted = sqlx::query_scalar::<_, i64>(
        "SELECT admitted_through FROM waiting_rooms WHERE sale_id = $1 FOR UPDATE",
    )
    .bind(sale.as_uuid())
    .fetch_optional(&mut *tx)
    .await
    .map_err(error)?;
    if admitted != Some(from) {
        return Ok(None);
    }
    // Every position up to the first has been handed out, and none beyond the second; joins
    // under way take higher ones.
    let (handed_out, highest) = sqlx::query_as::<_, (Option<i64>, Option<i64>)>(
        "SELECT MIN(next_position) - 1, MAX(next_position - stride)
         FROM waiting_room_counters WHERE sale_id = $1",
    )
    .bind(sale.as_uuid())
    .fetch_one(&mut *tx)
    .await
    .map_err(error)?;
    let (handed_out, to) = (handed_out.unwrap_or(from), to.min(highest.unwrap_or(from)));
    let through = if to <= from {
        from
    } else if handed_out > from {
        to.min(handed_out)
    } else {
        seal(&mut tx, sale, from, to).await?
    };
    if through <= from {
        return Ok(None);
    }
    sqlx::query("UPDATE waiting_rooms SET admitted_through = $2 WHERE sale_id = $1")
        .bind(sale.as_uuid())
        .bind(through)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
    tx.commit().await.map_err(error)?;
    u64::try_from(through)
        .map(Some)
        .map_err(StoreError::backend)
}

/// Admission has caught up and the next position is still free: nobody joined since. Moves
/// the counters behind `to` that no join holds past the new mark (the positions they skip are
/// never handed out), and returns the mark: `to`, or just below the lowest position of a
/// counter a join holds, which that join will hand out. Never waits for a join.
async fn seal(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    sale: SaleId,
    from: i64,
    to: i64,
) -> StoreResult<i64> {
    let ours = sqlx::query_scalar::<_, i32>(
        "SELECT counter FROM waiting_room_counters
         WHERE sale_id = $1 AND next_position <= $2 FOR UPDATE SKIP LOCKED",
    )
    .bind(sale.as_uuid())
    .bind(to)
    .fetch_all(&mut **tx)
    .await
    .map_err(error)?;
    let held = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MIN(next_position) FROM waiting_room_counters
         WHERE sale_id = $1 AND next_position <= $2 AND NOT (counter = ANY($3))",
    )
    .bind(sale.as_uuid())
    .bind(to)
    .bind(&ours)
    .fetch_one(&mut **tx)
    .await
    .map_err(error)?;
    let through = held.map_or(to, |next| to.min(next - 1));
    if through > from {
        // Only counters locked above reach this far: the others are past `through`.
        sqlx::query(
            "UPDATE waiting_room_counters
             SET next_position = next_position + stride * (($2 - next_position) / stride + 1)
             WHERE sale_id = $1 AND next_position <= $2",
        )
        .bind(sale.as_uuid())
        .bind(through)
        .execute(&mut **tx)
        .await
        .map_err(error)?;
    }
    Ok(through)
}

/// The position `account` holds in a sale's waiting room, if it joined.
async fn entry(pool: &PgPool, sale: SaleId, account: AccountId) -> StoreResult<Option<u64>> {
    let position = sqlx::query_scalar::<_, i64>(
        "SELECT position FROM waiting_room_entries WHERE sale_id = $1 AND account_id = $2",
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
