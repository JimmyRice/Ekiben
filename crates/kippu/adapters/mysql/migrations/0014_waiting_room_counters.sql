-- A waiting room hands out positions from 128 counter rows instead of one. Counter c hands out
-- c + 1, c + 1 + 128, …; a join takes the lowest position of a counter no other join holds
-- (FOR UPDATE SKIP LOCKED through the next_position index), so joins running together seldom
-- wait for each other and positions stay all but consecutive. Rooms that exist continue after
-- their last position.
--
-- last_position goes: the highest position handed out is read from the counters. Dropping it
-- also stops instances of the previous release from handing out positions the counters would
-- hand out again; their joins fail until they are replaced, so upgrade outside a sale's opening.

CREATE TABLE waiting_room_counters (
    sale_id       BINARY(16) NOT NULL,
    counter       INT        NOT NULL,
    next_position BIGINT     NOT NULL,
    stride        BIGINT     NOT NULL,
    PRIMARY KEY (sale_id, counter),
    KEY waiting_room_counters_by_next (sale_id, next_position),
    FOREIGN KEY (sale_id) REFERENCES waiting_rooms (sale_id)
);

INSERT INTO waiting_room_counters (sale_id, counter, next_position, stride)
WITH RECURSIVE counters (counter) AS (
    SELECT 0 UNION ALL SELECT counter + 1 FROM counters WHERE counter < 127
)
SELECT r.sale_id, counters.counter, r.last_position + counters.counter + 1, 128
FROM waiting_rooms r CROSS JOIN counters;

-- The CHECK (admitted_through <= last_position) of 0001_initial.sql, as MySQL named it.
ALTER TABLE waiting_rooms DROP CHECK waiting_rooms_chk_1, DROP COLUMN last_position;
