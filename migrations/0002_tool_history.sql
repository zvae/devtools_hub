-- 每个工具独立保存输入与输出历史，用户可在设置中配置每工具保留条数。
CREATE TABLE IF NOT EXISTS tool_history (
    id INTEGER PRIMARY KEY,
    tool_id TEXT NOT NULL,
    input TEXT NOT NULL,
    output TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tool_history_tool_created
ON tool_history(tool_id, created_at DESC, id DESC);
