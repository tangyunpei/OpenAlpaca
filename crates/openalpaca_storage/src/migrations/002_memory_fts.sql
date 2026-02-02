-- FTS5 全文索引
CREATE VIRTUAL TABLE memory_fts USING fts5(
    content,
    agent_id UNINDEXED,
    content='memory',
    content_rowid='id'
);

-- INSERT 触发器
CREATE TRIGGER memory_ai AFTER INSERT ON memory BEGIN
    INSERT INTO memory_fts(rowid, content, agent_id)
    VALUES (NEW.id, NEW.content, NEW.agent_id);
END;

-- DELETE 触发器
CREATE TRIGGER memory_ad AFTER DELETE ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, agent_id)
    VALUES ('delete', OLD.id, OLD.content, OLD.agent_id);
END;

-- UPDATE 触发器
CREATE TRIGGER memory_au AFTER UPDATE ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, agent_id)
    VALUES ('delete', OLD.id, OLD.content, OLD.agent_id);
    INSERT INTO memory_fts(rowid, content, agent_id)
    VALUES (NEW.id, NEW.content, NEW.agent_id);
END;

-- Update schema version
UPDATE schema_version SET version = 2;
