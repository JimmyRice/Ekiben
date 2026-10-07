-- An event's page content, in whatever form the deployment's clients render.

ALTER TABLE events ADD COLUMN content TEXT NOT NULL DEFAULT '';
