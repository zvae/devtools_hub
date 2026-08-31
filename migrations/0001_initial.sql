CREATE TABLE IF NOT EXISTS schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS plugin (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    runtime TEXT NOT NULL,
    path TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS command (
    id TEXT PRIMARY KEY,
    plugin_id TEXT,
    title TEXT NOT NULL,
    keywords TEXT,
    source TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS execution_history (
    id INTEGER PRIMARY KEY,
    command_id TEXT NOT NULL,
    input_hash TEXT,
    executed_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_execution_history_command_id
ON execution_history(command_id);

CREATE INDEX IF NOT EXISTS idx_execution_history_executed_at
ON execution_history(executed_at);

CREATE TABLE IF NOT EXISTS clipboard_item (
    id INTEGER PRIMARY KEY,
    content TEXT NOT NULL,
    content_type TEXT NOT NULL,
    hash TEXT NOT NULL UNIQUE,
    source_app TEXT,
    favorite INTEGER NOT NULL DEFAULT 0,
    sensitive INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    expires_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_clipboard_created_at
ON clipboard_item(created_at);

CREATE INDEX IF NOT EXISTS idx_clipboard_content_type
ON clipboard_item(content_type);

CREATE VIRTUAL TABLE IF NOT EXISTS clipboard_item_fts
USING fts5(content, content='clipboard_item', content_rowid='id');

CREATE TRIGGER IF NOT EXISTS clipboard_item_ai
AFTER INSERT ON clipboard_item
BEGIN
    INSERT INTO clipboard_item_fts(rowid, content)
    VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS clipboard_item_ad
AFTER DELETE ON clipboard_item
BEGIN
    INSERT INTO clipboard_item_fts(clipboard_item_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS clipboard_item_au
AFTER UPDATE ON clipboard_item
BEGIN
    INSERT INTO clipboard_item_fts(clipboard_item_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
    INSERT INTO clipboard_item_fts(rowid, content)
    VALUES (new.id, new.content);
END;
