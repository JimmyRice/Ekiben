-- An event's precise address, for maps and navigation. All NULL when the event has none
-- (online, or the place is not settled yet); `address_street` is set whenever
-- `address_country` is.

ALTER TABLE events ADD COLUMN address_country TEXT;
ALTER TABLE events ADD COLUMN address_region TEXT;
ALTER TABLE events ADD COLUMN address_locality TEXT;
ALTER TABLE events ADD COLUMN address_postal_code TEXT;
ALTER TABLE events ADD COLUMN address_street TEXT;
ALTER TABLE events ADD COLUMN latitude REAL;
ALTER TABLE events ADD COLUMN longitude REAL;
