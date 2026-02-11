-- Migration 017: Upgrade memory_vec from 384-dim to 768-dim embeddings
-- Switches to intfloat/multilingual-e5-base model. All existing embeddings
-- are dropped; the background indexer will re-embed automatically.

DROP TABLE IF EXISTS memory_vec;

CREATE VIRTUAL TABLE IF NOT EXISTS memory_vec USING vec0(
    memory_id INTEGER PRIMARY KEY,
    embedding float[768]
);

UPDATE schema_version SET version = 17 WHERE version = 16;
