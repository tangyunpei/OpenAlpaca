-- FTS5 全文索引 (幂等版本)
-- 先删除已有触发器和表，确保可重复执行

DROP TRIGGER IF EXISTS memory_ai;
DROP TRIGGER IF EXISTS memory_ad;
DROP TRIGGER IF EXISTS memory_au;
DROP TABLE IF EXISTS memory_fts;

-- 创建 FTS5 虚拟表
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

-- UPDATE 触发器 (先删旧索引，再插新索引)
CREATE TRIGGER memory_au AFTER UPDATE ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, agent_id)
    VALUES ('delete', OLD.id, OLD.content, OLD.agent_id);
    INSERT INTO memory_fts(rowid, content, agent_id)
    VALUES (NEW.id, NEW.content, NEW.agent_id);
END;

-- Update schema version
UPDATE schema_version SET version = 2;
