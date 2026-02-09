-- Migration 013: Vector search support via sqlite-vec
-- Adds a vec0 virtual table for semantic similarity search over memory embeddings.
--
-- Dimension: 384 (matches sentence-transformers/all-MiniLM-L6-v2 and similar
-- lightweight models suitable for local inference via Ollama). If a different
-- embedding model is used later, create a new vec0 table with the appropriate
-- dimension rather than altering this one (vec0 tables are append-only by design).

CREATE VIRTUAL TABLE IF NOT EXISTS memory_vec USING vec0(
    memory_id INTEGER PRIMARY KEY,
    embedding float[384]
);

UPDATE schema_version SET version = 13 WHERE version = 12;
