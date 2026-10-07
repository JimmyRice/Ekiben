-- An event's precise address, for maps and navigation. All NULL when the event has none
-- (online, or the place is not settled yet).

ALTER TABLE events
    ADD COLUMN address_country     TEXT,
    ADD COLUMN address_region      TEXT,
    ADD COLUMN address_locality    TEXT,
    ADD COLUMN address_postal_code TEXT,
    ADD COLUMN address_street      TEXT,
    ADD COLUMN latitude            DOUBLE PRECISION,
    ADD COLUMN longitude           DOUBLE PRECISION,
    ADD CONSTRAINT events_address_has_street
        CHECK ((address_country IS NULL) = (address_street IS NULL));
