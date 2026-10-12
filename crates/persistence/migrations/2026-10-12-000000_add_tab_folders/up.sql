CREATE TABLE tab_folders (
  id INTEGER PRIMARY KEY NOT NULL,
  directory TEXT NOT NULL,
  name TEXT,
  color TEXT,
  custom_color TEXT,
  icon TEXT,
  collapsed BOOLEAN NOT NULL DEFAULT FALSE
);

ALTER TABLE tab_groups ADD COLUMN directory TEXT;
ALTER TABLE tab_groups ADD COLUMN custom_color TEXT;
ALTER TABLE tab_groups ADD COLUMN icon TEXT;
ALTER TABLE tabs ADD COLUMN stays_ungrouped BOOLEAN;
