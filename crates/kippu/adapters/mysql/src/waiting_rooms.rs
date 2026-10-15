//! Waiting rooms. A room hands out positions from [`COUNTERS`] counter rows: counter `c` hands
//! out `c + 1`, `c + 1 + COUNTERS`, … (a room from before the counters continues after its
//! last position), and a join takes the lowest position of a counter no other join holds at
//! that moment (`FOR UPDATE SKIP LOCKED`). Joins running together seldom wait for each other,
//! and positions stay all but consecutive: only those being handed out at the same moment can
//! come out of order.
//!
//! Counters are read through `(sale_id, next_position)`, forced: with a filesort InnoDB would
//! lock every counter it reads before applying `LIMIT`.

use kippu_domain::admission::WaitingRoom;
use kippu_domain::{AccountId, SaleId};
use kippu_store::{StoreError, StoreResult};
use sqlx::{MySqlConnection, MySqlPool};
use uuid::Uuid;

use crate::{error, is_duplicate};

/// Counter rows per waiting room. Joins wait for each other only when more of them run at
/// once, across all instances, than there are counters.
pub(crate) const COUNTERS: i32 = 128;

/// The counter with the lowest next position that can be locked, and that position. `wait`
/// waits for the lowest counter instead of skipping those other joins hold.
fn pick(wait: bool) -> sqlx::AssertSqlSafe<String> {
    let lock = if wait {
        "FOR UPDATE"
    } else {
        "FOR UPDATE SKIP LOCKED"
    };
    sqlx::AssertSqlSafe(format!(
        "SELECT counter, next_position FROM waiting_room_counters
         FORCE INDEX (waiting_room_counters_by_next)
         WHERE sale_id = ? ORDER BY next_position LIMIT 1 {lock}"
    ))
}

/// Makes a sale's waiting room and its counters, unless they exist.
pub(crate) async fn create(connection: &mut MySqlConnection, sale: SaleId) -> StoreResult<()> {
    let room = sqlx::query("INSERT INTO waiting_rooms (sale_id, admitted_through) VALUES (?, 0)")
        .bind(sale.as_uuid())
        .execute(&mut *connection)
        .await;
    let counters = sqlx::query(
        "INSERT INTO waiting_room_counters (sale_id, counter, next_position, stride)
         WITH RECURSIVE counters (counter) AS (
             SELECT 0 UNION ALL SELECT counter + 1 FROM counters WHERE counter + 1 < ?
         )
         SELECT ?, counter, counter + 1, ? FROM counters",
    )
    .bind(COUNTERS)
    .bind(sale.as_uuid())
    .bind(COUNTERS)
    .execute(&mut *connection)
    .await;
    for made in [room, counters] {
        match made {
            Ok(_) => {}
            // Another join made them first.
            Err(failure) if is_duplicate(&failure) => {}
            Err(failure) => return Err(error(failure)),
        }
    }
    Ok(())
}

pub(crate) async fn join(pool: &MySqlPool, sale: SaleId, account: AccountId) -> StoreResult<u64> {
    if let Some(position) = entry(pool, sale, account).await? {
        return Ok(position);
    }
    // Usually a counter no other join holds. Failing that, every counter is held at this
    // moment, or the sale's room was never made (the sale took a waiting room after it was
    // created): then wait for the lowest counter, after making the room if need be.
    for (wait, make_room) in [(false, false), (true, false), (true, true)] {
        if make_room {
            let mut tx = pool.begin().await.map_err(error)?;
            create(&mut tx, sale).await?;
            tx.commit().await.map_err(error)?;
        }
        let mut tx = pool.begin().await.map_err(error)?;
        let picked = sqlx::query_as::<_, (i32, i64)>(pick(wait))
            .bind(sale.as_uuid())
            .fetch_optional(&mut *tx)
            .await
            .map_err(error)?;
        let Some((counter, position)) = picked else {
            continue;
        };
        sqlx::query(
            "UPDATE waiting_room_counters SET next_position = next_position + stride
             WHERE sale_id = ? AND counter = ?",
        )
        .bind(sale.as_uuid())
        .bind(counter)
        .execute(&mut *tx)
        .await
        .map_err(error)?;
        let entered = sqlx::query(
            "INSERT INTO waiting_room_entries (sale_id, account_id, position) VALUES (?, ?, ?)",
        )
        .bind(sale.as_uuid())
        .bind(account.as_uuid())
        .bind(position)
        .execute(&mut *tx)
        .await;
        return match entered {
            Ok(_) => {
                tx.commit().await.map_err(error)?;
                u64::try_from(position).map_err(StoreError::backend)
            }
            // The same account joining at the same moment entered first: the counter is rolled
            // back and its position stands.
            Err(failure) if is_duplicate(&failure) => {
                drop(tx);
                entry(pool, sale, account)
                    .await?
                    .ok_or_else(|| StoreError::backend("a waiting room entry vanished"))
            }
            Err(failure) => Err(error(failure)),
        };
    }
    Err(StoreError::backend("a waiting room has no counters"))
}

pub(crate) async fn room(pool: &MySqlPool, sale: SaleId) -> StoreResult<WaitingRoom> {
    let row = sqlx::query_as::<_, (i64, Option<i64>)>(
        "SELECT admitted_through,
                (SELECT MAX(next_position - stride) FROM waiting_room_counters WHERE sale_id = ?)
         FROM waiting_rooms WHERE sale_id = ?",
    )
    .bind(sale.as_uuid())
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

pub(crate) async fn pending(pool: &MySqlPool) -> StoreResult<Vec<SaleId>> {
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT sale_id FROM waiting_rooms r
         WHERE admitted_through < (SELECT MAX(next_position - stride) FROM waiting_room_counters c
                                   WHERE c.sale_id = r.sale_id)
         ORDER BY sale_id",
    )
    .fetch_all(pool)
    .await
    .map_err(error)?;
    Ok(ids.into_iter().map(Into::into).collect())
}

pub(crate) async fn advance(
    pool: &MySqlPool,
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
        "SELECT admitted_through FROM waiting_rooms WHERE sale_id = ? FOR UPDATE",
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
         FROM waiting_room_counters WHERE sale_id = ?",
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
    sqlx::query("UPDATE waiting_rooms SET admitted_through = ? WHERE sale_id = ?")
        .bind(through)
        .bind(sale.as_uuid())
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
    connection: &mut MySqlConnection,
    sale: SaleId,
    from: i64,
    to: i64,
) -> StoreResult<i64> {
    let ours = sqlx::query_scalar::<_, i32>(
        "SELECT counter FROM waiting_room_counters FORCE INDEX (waiting_room_counters_by_next)
         WHERE sale_id = ? AND next_position <= ? FOR UPDATE SKIP LOCKED",
    )
    .bind(sale.as_uuid())
    .bind(to)
    .fetch_all(&mut *connection)
    .await
    .map_err(error)?;
    let behind = sqlx::query_as::<_, (i32, i64)>(
        "SELECT counter, next_position FROM waiting_room_counters
         WHERE sale_id = ? AND next_position <= ?",
    )
    .bind(sale.as_uuid())
    .bind(to)
    .fetch_all(&mut *connection)
    .await
    .map_err(error)?;
    let held = behind
        .iter()
        .filter(|(counter, _)| !ours.contains(counter))
        .map(|&(_, next)| next)
        .min();
    let through = held.map_or(to, |next| to.min(next - 1));
    if through > from {
        // Only counters locked above reach this far: the others are past `through`.
        sqlx::query(
            "UPDATE waiting_room_counters FORCE INDEX (waiting_room_counters_by_next)
             SET next_position = next_position + stride * ((? - next_position) DIV stride + 1)
             WHERE sale_id = ? AND next_position <= ?",
        )
        .bind(through)
        .bind(sale.as_uuid())
        .bind(through)
        .execute(&mut *connection)
        .await
        .map_err(error)?;
    }
    Ok(through)
}

/// The position `account` holds in a sale's waiting room, if it joined.
async fn entry(pool: &MySqlPool, sale: SaleId, account: AccountId) -> StoreResult<Option<u64>> {
    let position = sqlx::query_scalar::<_, i64>(
        "SELECT position FROM waiting_room_entries WHERE sale_id = ? AND account_id = ?",
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
