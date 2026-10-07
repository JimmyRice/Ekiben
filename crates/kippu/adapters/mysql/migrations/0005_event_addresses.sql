-- An event's precise address, for maps and navigation. All NULL when the event has none
-- (online, or the place is not settled yet).

ALTER TABLE events
    ADD COLUMN address_country     CHAR(2)      NULL,
    ADD COLUMN address_region      VARCHAR(200) NULL,
    ADD COLUMN address_locality    VARCHAR(200) NULL,
    ADD COLUMN address_postal_code VARCHAR(32)  NULL,
    ADD COLUMN address_street      VARCHAR(500) NULL,
    ADD COLUMN latitude            DOUBLE       NULL,
    ADD COLUMN longitude           DOUBLE       NULL,
    ADD CONSTRAINT events_address_has_street
        CHECK ((address_country IS NULL) = (address_street IS NULL));
