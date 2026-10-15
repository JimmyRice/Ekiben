-- no-transaction
--
-- A waiting room hands out positions from counter rows: the other adapters keep many per room
-- so that joins running together do not wait for each other. SQLite has one writer, so a room
-- keeps one counter (stride 1), which continues after the room's last position.
--
-- last_position goes: the highest position handed out is read from the counter. SQLite cannot
-- drop a column a CHECK uses, so `waiting_rooms` is rebuilt as its documentation prescribes:
-- with foreign keys off, inside one transaction.

PRAGMA foreign_keys = OFF;

BEGIN IMMEDIATE;

CREATE TABLE waiting_rooms_rebuilt (
    sale_id          BLOB    PRIMARY KEY REFERENCES sales (id),
    admitted_through INTEGER NOT NULL
) STRICT;

INSERT INTO waiting_rooms_rebuilt (sale_id, admitted_through)
SELECT sale_id, admitted_through FROM waiting_rooms;

CREATE TABLE waiting_room_counters (
    sale_id       BLOB    NOT NULL REFERENCES waiting_rooms (sale_id),
    counter       INTEGER NOT NULL,
    next_position INTEGER NOT NULL,
    stride        INTEGER NOT NULL,
    PRIMARY KEY (sale_id, counter)
) STRICT;

INSERT INTO waiting_room_counters (sale_id, counter, next_position, stride)
SELECT sale_id, 0, last_position + 1, 1 FROM waiting_rooms;

DROP TABLE waiting_rooms;

ALTER TABLE waiting_rooms_rebuilt RENAME TO waiting_rooms;

COMMIT;

PRAGMA foreign_keys = ON;
