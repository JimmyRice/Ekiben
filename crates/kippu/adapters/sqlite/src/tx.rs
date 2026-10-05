use async_trait::async_trait;
use kippu_store::{
    InventoryTx, OutboxTx, PaymentsTx, PurchasesTx, ReservationsTx, StoreResult, StoreTx, TicketsTx,
};
use sqlx::{Sqlite, Transaction};

use crate::error;

/// A write transaction on the single writer connection, opened with `BEGIN IMMEDIATE` so it
/// holds SQLite's write lock from the start.
pub(crate) struct SqliteTx {
    pub(crate) conn: Transaction<'static, Sqlite>,
}

impl SqliteTx {
    pub(crate) fn new(conn: Transaction<'static, Sqlite>) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl StoreTx for SqliteTx {
    fn purchases(&mut self) -> &mut dyn PurchasesTx {
        self
    }

    fn inventory(&mut self) -> &mut dyn InventoryTx {
        self
    }

    fn reservations(&mut self) -> &mut dyn ReservationsTx {
        self
    }

    fn payments(&mut self) -> &mut dyn PaymentsTx {
        self
    }

    fn tickets(&mut self) -> &mut dyn TicketsTx {
        self
    }

    fn outbox(&mut self) -> &mut dyn OutboxTx {
        self
    }

    async fn commit(self: Box<Self>) -> StoreResult<()> {
        self.conn.commit().await.map_err(error)
    }
}
