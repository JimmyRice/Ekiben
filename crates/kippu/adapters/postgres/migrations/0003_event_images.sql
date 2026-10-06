-- What is known about event images; the bytes are in object storage.

CREATE TABLE event_images (
    id         UUID        PRIMARY KEY,
    event_id   UUID        NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    format     TEXT        NOT NULL CHECK (format IN ('image/png', 'image/jpeg', 'image/webp',
                                                      'image/avif')),
    size_bytes BIGINT      NOT NULL CHECK (size_bytes >= 0),
    position   BIGINT      NOT NULL,
    created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX event_images_by_event ON event_images (event_id, position);
