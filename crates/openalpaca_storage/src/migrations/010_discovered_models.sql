-- Migration 010: Discovered Models Cache
-- Stores models discovered from provider APIs for offline resilience.
-- On refresh: if provider returns models, replace that provider's rows.
-- If provider returns empty (API down), keep existing rows.

CREATE TABLE IF NOT EXISTS discovered_models (
    model_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    input_price_per_million REAL DEFAULT 0,
    output_price_per_million REAL DEFAULT 0,
    context_window INTEGER DEFAULT 0,
    discovered_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (model_id)
);

CREATE INDEX IF NOT EXISTS idx_discovered_models_provider ON discovered_models(provider);

UPDATE schema_version SET version = 10 WHERE version = 9;
