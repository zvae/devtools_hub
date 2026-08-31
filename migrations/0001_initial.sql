-- 记录已执行的数据库迁移版本，避免重复执行迁移脚本。
CREATE TABLE IF NOT EXISTS schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

-- 应用设置表：主题、语言、快捷键等键值配置统一存放在这里。
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

-- 插件注册表：阶段 0/1 先保留结构，后续接入插件安装和启停。
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

-- 命令注册表：内置工具和插件命令都可以登记为可搜索命令。
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

-- 命令执行历史：只保存命令和输入哈希，不保存原始敏感输入。
CREATE TABLE IF NOT EXISTS execution_history (
    id INTEGER PRIMARY KEY,
    command_id TEXT NOT NULL,
    input_hash TEXT,
    executed_at INTEGER NOT NULL
);

-- 执行历史按命令和时间建索引，便于后续统计常用工具。
CREATE INDEX IF NOT EXISTS idx_execution_history_command_id
ON execution_history(command_id);

CREATE INDEX IF NOT EXISTS idx_execution_history_executed_at
ON execution_history(executed_at);

-- 剪贴板历史表：保存内容、类型、来源窗口、收藏和敏感标记。
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

-- 剪贴板列表常按创建时间和内容类型过滤。
CREATE INDEX IF NOT EXISTS idx_clipboard_created_at
ON clipboard_item(created_at);

CREATE INDEX IF NOT EXISTS idx_clipboard_content_type
ON clipboard_item(content_type);

-- FTS5 虚拟表用于剪贴板全文搜索。
CREATE VIRTUAL TABLE IF NOT EXISTS clipboard_item_fts
USING fts5(content, content='clipboard_item', content_rowid='id');

-- 插入剪贴板记录后同步写入全文索引。
CREATE TRIGGER IF NOT EXISTS clipboard_item_ai
AFTER INSERT ON clipboard_item
BEGIN
    INSERT INTO clipboard_item_fts(rowid, content)
    VALUES (new.id, new.content);
END;

-- 删除剪贴板记录时同步删除全文索引。
CREATE TRIGGER IF NOT EXISTS clipboard_item_ad
AFTER DELETE ON clipboard_item
BEGIN
    INSERT INTO clipboard_item_fts(clipboard_item_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
END;

-- 更新剪贴板记录时重建对应全文索引。
CREATE TRIGGER IF NOT EXISTS clipboard_item_au
AFTER UPDATE ON clipboard_item
BEGIN
    INSERT INTO clipboard_item_fts(clipboard_item_fts, rowid, content)
    VALUES ('delete', old.id, old.content);
    INSERT INTO clipboard_item_fts(rowid, content)
    VALUES (new.id, new.content);
END;
