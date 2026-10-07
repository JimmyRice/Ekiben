use async_trait::async_trait;
use kippu_store::{
    InventoryTx, OutboxTx, PaymentsTx, PurchasesTx, ReservationsTx, StoreResult, StoreTx, TicketsTx,
};
use sqlx::{Postgres, Transaction};

use crate::error;

/// A transaction at the default isolation level, READ COMMITTED. Rows that are read and then
/// changed are locked explicitly.
pub(crate) struct PostgresTx {
    pub(crate) conn: Transaction<'static, Postgres>,
}

impl PostgresTx {
    pub(crate) fn new(conn: Transaction<'static, Postgres>) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl StoreTx for PostgresTx {
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
