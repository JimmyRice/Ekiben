-- An event's page content, in whatever form the deployment's clients render. MEDIUMTEXT, since
-- TEXT holds only 64 KiB.

ALTER TABLE events ADD COLUMN content MEDIUMTEXT NOT NULL DEFAULT ('');
