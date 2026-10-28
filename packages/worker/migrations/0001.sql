-- Apply explicitly with the deployment's D1 migration command, never at request startup.
CREATE TABLE aether_schema (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  version INTEGER NOT NULL CHECK (version >= 1)
);
INSERT INTO aether_schema (singleton, version) VALUES (1, 1);
CREATE TABLE aether_documents (
  application_id TEXT NOT NULL,
  tenant_key TEXT NOT NULL,
  subject_id TEXT NOT NULL,
  owner_key TEXT NOT NULL,
  collection TEXT NOT NULL,
  key TEXT NOT NULL,
  revision INTEGER NOT NULL CHECK (revision >= 1),
  value_json TEXT,
  blob_key TEXT,
  deleted INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (application_id, tenant_key, subject_id, collection, key),
  CHECK ((deleted = 0 AND ((value_json IS NOT NULL AND json_valid(value_json) AND blob_key IS NULL) OR (value_json IS NULL AND blob_key IS NOT NULL))) OR (deleted = 1 AND value_json IS NULL AND blob_key IS NULL))
) WITHOUT ROWID;
CREATE INDEX aether_documents_list ON aether_documents (application_id, tenant_key, subject_id, collection, deleted, key);
