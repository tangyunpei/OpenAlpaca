-- Schema version tracking
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY
);
INSERT INTO schema_version(version) VALUES (1);

-- Agent 配置
CREATE TABLE agent (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    persona TEXT,
    config TEXT,  -- JSON
    created_at TEXT DEFAULT (datetime('now'))
);

-- 事件日志 (审计)
CREATE TABLE event_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT DEFAULT (datetime('now')),
    agent_id TEXT,
    event_type TEXT NOT NULL,
    detail TEXT,  -- JSON
    result TEXT
);

CREATE INDEX idx_event_log_timestamp ON event_log(timestamp);
CREATE INDEX idx_event_log_agent ON event_log(agent_id);
CREATE INDEX idx_event_log_type ON event_log(event_type);

-- 记忆存储
CREATE TABLE memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL,  -- 'user' | 'agent' | 'system'
    content TEXT NOT NULL,
    timestamp TEXT DEFAULT (datetime('now')),
    metadata TEXT,  -- JSON, 可选扩展字段
    FOREIGN KEY (agent_id) REFERENCES agent(id)
);

CREATE INDEX idx_memory_agent ON memory(agent_id);
CREATE INDEX idx_memory_timestamp ON memory(timestamp);
