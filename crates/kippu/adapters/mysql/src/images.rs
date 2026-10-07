use async_trait::async_trait;
use kippu_domain::image::EventImage;
use kippu_domain::{EventId, ImageId};
use kippu_store::{ImageStore, StoreError, StoreResult};

use crate::convert::{ImageRow, all, micros, optional};
use crate::{MySqlStore, error};

const IMAGE_COLUMNS: &str = "id, event_id, format, size_bytes, position, created_at";

#[async_trait]
impl ImageStore for MySqlStore {
    async fn insert_event_image(&self, image: &EventImage) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO event_images (id, event_id, format, size_bytes, position, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(image.id.as_uuid())
        .bind(image.event_id.as_uuid())
        .bind(image.format.media_type())
        .bind(i64::try_from(image.size_bytes).map_err(StoreError::backend)?)
        .bind(image.position)
        .bind(micros(image.created_at))
        .execute(&self.pool)
        .await
        .map_err(error)?;
        Ok(())
    }

    async fn event_image(&self, id: ImageId) -> StoreResult<Option<EventImage>> {
        let row = sqlx::query_as::<_, ImageRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {IMAGE_COLUMNS} FROM event_images WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(error)?;
        optional(row)
    }

    async fn event_images(&self, event: EventId) -> StoreResult<Vec<EventImage>> {
        let rows = sqlx::query_as::<_, ImageRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {IMAGE_COLUMNS} FROM event_images WHERE event_id = ? ORDER BY position, id"
        )))
        .bind(event.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(error)?;
        all(rows)
    }

    async fn set_image_positions(&self, event: EventId, order: &[ImageId]) -> StoreResult<()> {
        let mut tx = self.pool.begin().await.map_err(error)?;
        for (index, id) in order.iter().enumerate() {
            sqlx::query("UPDATE event_images SET position = ? WHERE id = ? AND event_id = ?")
                .bind(i64::try_from(index).map_err(StoreError::backend)?)
                .bind(id.as_uuid())
                .bind(event.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(error)?;
        }
        tx.commit().await.map_err(error)
    }

    async fn delete_event_image(&self, id: ImageId) -> StoreResult<bool> {
        let result = sqlx::query("DELETE FROM event_images WHERE id = ?")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(error)?;
        Ok(result.rows_affected() == 1)
    }
}
