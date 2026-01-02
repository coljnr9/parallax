-- Track schema version and migration metadata
CREATE TABLE IF NOT EXISTS schema_metadata (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP NOT NULL
);

-- Insert initial schema version
INSERT OR IGNORE INTO schema_metadata (key, value) VALUES ('schema_version', '1.0.0');

