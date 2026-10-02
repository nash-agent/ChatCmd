-- Existing projects remain private to explicitly bound conversations.
ALTER TABLE workspace_projects ADD COLUMN allow_all_conversations INTEGER NOT NULL DEFAULT 0 CHECK (allow_all_conversations IN (0, 1));
-- Capture the approved canonical directory, rather than following a changed symlink later.
ALTER TABLE workspace_projects ADD COLUMN global_access_path TEXT;
UPDATE schema_version SET version=27 WHERE singleton_id=1;
UPDATE app_metadata SET value = '27' WHERE key = 'schema_version';
