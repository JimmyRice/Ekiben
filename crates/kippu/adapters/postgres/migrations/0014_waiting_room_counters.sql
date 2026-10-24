-- A waiting room hands out positions from 128 counter rows instead of one. Counter c hands out
-- c + 1, c + 1 + 128, …; a join takes the lowest position of a counter no other join holds
-- (FOR UPDATE SKIP LOCKED), so joins running together seldom wait for each other and positions
-- stay all but consecutive. Rooms that exist continue after their last position.
--
-- last_position goes: the highest position handed out is read from the counters. Dropping it
-- also stops instances of the previous release from handing out positions the counters would
-- hand out again; their joins fail until they are replaced, so upgrade outside a sale's opening.

CREATE TABLE waiting_room_counters (
    sale_id       UUID   NOT NULL REFERENCES waiting_rooms (sale_id),
    counter       INT    NOT NULL,
    next_position BIGINT NOT NULL,
    stride        BIGINT NOT NULL,
    PRIMARY KEY (sale_id, counter)
);

INSERT INTO waiting_room_counters (sale_id, counter, next_position, stride)
SELECT sale_id, counter, last_position + counter + 1, 128
FROM waiting_rooms CROSS JOIN generate_series(0, 127) AS counter;

ALTER TABLE waiting_rooms DROP COLUMN last_position;
