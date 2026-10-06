-- What is known about event images; the bytes are in object storage.

CREATE TABLE event_images (
    id         BINARY(16)   PRIMARY KEY,
    event_id   BINARY(16)   NOT NULL,
    format     VARCHAR(16)  NOT NULL CHECK (format IN ('image/png', 'image/jpeg', 'image/webp',
                                                       'image/avif')),
    size_bytes BIGINT       NOT NULL CHECK (size_bytes >= 0),
    position   INT UNSIGNED NOT NULL,
    created_at BIGINT       NOT NULL,
    INDEX event_images_by_event (event_id, position),
    FOREIGN KEY (event_id) REFERENCES events (id) ON DELETE CASCADE
);
