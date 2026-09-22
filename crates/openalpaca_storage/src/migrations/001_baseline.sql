-- Initial application schema, version 1.
-- Database::open creates and records schema_migrations in the same transaction.
-- Append future changes as new migrations; keep version bookkeeping out of SQL.

-- Identity and configuration

CREATE TABLE global_user (
    id TEXT PRIMARY KEY,
    display_name TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE external_identity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    provider_user_id TEXT NOT NULL,
    global_user_id TEXT,
    display_name TEXT,
    metadata TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    linked_at TEXT,
    FOREIGN KEY (global_user_id) REFERENCES global_user(id),
    UNIQUE(provider, provider_user_id)
);

CREATE INDEX idx_external_identity_global_user ON external_identity(global_user_id);
CREATE INDEX idx_external_identity_provider ON external_identity(provider, provider_user_id);

CREATE TABLE conversation_map (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    provider_conversation_id TEXT NOT NULL,
    global_user_id TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    lane_key TEXT,
    FOREIGN KEY (global_user_id) REFERENCES global_user(id),
    UNIQUE(provider, provider_conversation_id)
);

CREATE INDEX idx_conversation_map_provider ON conversation_map(provider, provider_conversation_id);

CREATE TABLE link_token (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token TEXT NOT NULL UNIQUE,
    global_user_id TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used_at TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    FOREIGN KEY (global_user_id) REFERENCES global_user(id)
);

CREATE INDEX idx_link_token_token ON link_token(token);

CREATE TABLE system_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    kind TEXT NOT NULL,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE preference (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    UNIQUE(user_id, key)
);

CREATE INDEX idx_preference_user ON preference(user_id);

-- Agents and tasks

CREATE TABLE agent (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    persona TEXT,
    config TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    description TEXT,
    icon TEXT,
    status TEXT DEFAULT 'idle',
    current_task_id TEXT,
    skills_json TEXT DEFAULT '[]',
    preset_json TEXT DEFAULT '{}',
    constraints_json TEXT,
    updated_at TEXT,
    llm_config_json TEXT,
    template_id TEXT NOT NULL DEFAULT ''
);

CREATE TABLE task (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'queued',
    priority INTEGER NOT NULL DEFAULT 0,
    progress_current INTEGER,
    progress_total INTEGER,
    result_summary TEXT,
    created_by TEXT NOT NULL,
    source_lane TEXT NOT NULL,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    completed_at TEXT,
    state_json TEXT,
    state_version INTEGER NOT NULL DEFAULT 0,
    outcome_json TEXT,
    outcome_kind TEXT,
    artifact_count INTEGER NOT NULL DEFAULT 0,
    workspace_id TEXT,
    source_task_id TEXT,
    session_id TEXT,
    unattended INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_task_created_by ON task(created_by);
CREATE INDEX idx_task_session ON task(session_id);
CREATE INDEX idx_task_source ON task(source_task_id);
CREATE INDEX idx_task_status ON task(status);
CREATE INDEX idx_task_workspace ON task(workspace_id);

CREATE TABLE task_agent_assignment (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    step_order INTEGER,
    started_at TEXT,
    completed_at TEXT,
    result_output TEXT
);

CREATE INDEX idx_task_agent_task ON task_agent_assignment(task_id);

CREATE TABLE agent_metrics (
    agent_id TEXT PRIMARY KEY REFERENCES agent(id) ON DELETE CASCADE,
    tasks_completed INTEGER DEFAULT 0,
    tasks_failed INTEGER DEFAULT 0,
    total_runtime_seconds INTEGER DEFAULT 0,
    average_runtime_seconds REAL DEFAULT 0,
    success_rate REAL DEFAULT 1.0,
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE agent_task_history (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agent(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    status TEXT NOT NULL,
    runtime_seconds INTEGER,
    completed_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX idx_agent_task_history ON agent_task_history(agent_id, completed_at DESC);

-- The blocked state is derived from pending confirmations rather than persisted.

CREATE TABLE subagent_span (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    template_id TEXT NOT NULL,
    agent_instance_id TEXT NOT NULL,
    label TEXT NOT NULL,
    objective TEXT,
    state TEXT NOT NULL DEFAULT 'running' CHECK (state IN ('running', 'done', 'failed', 'blocked', 'cancelled')),
    detail TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    duration_ms INTEGER,
    output_preview TEXT
);

CREATE UNIQUE INDEX idx_subagent_span_label ON subagent_span(task_id, label);
CREATE INDEX idx_subagent_span_state ON subagent_span(state);
CREATE INDEX idx_subagent_span_task ON subagent_span(task_id, started_at);

-- Sessions and messages

-- Only one active session per lane; archived sessions can share the lane.

CREATE TABLE session (
    id TEXT PRIMARY KEY,
    lane_key TEXT NOT NULL,
    source TEXT NOT NULL,
    title TEXT DEFAULT '',
    workspace_id TEXT,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
    message_count INTEGER DEFAULT 0,
    last_message_at TEXT,
    summary TEXT NOT NULL DEFAULT '',
    summary_version INTEGER NOT NULL DEFAULT 0,
    last_summarized_message_id INTEGER NOT NULL DEFAULT 0,
    summary_updated_at TEXT,
    ended_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE UNIQUE INDEX idx_session_active_lane ON session(lane_key) WHERE status = 'active';
CREATE INDEX idx_session_source ON session(source);
CREATE INDEX idx_session_updated ON session(updated_at DESC);
CREATE INDEX idx_session_workspace ON session(workspace_id, updated_at DESC);

-- task_id deliberately has no foreign key: a transcript survives task pruning.

CREATE TABLE conversation_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    lane_key TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    model TEXT,
    tokens_in INTEGER,
    tokens_out INTEGER,
    duration_ms INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    source TEXT,
    content_json TEXT,
    display_text TEXT,
    task_id TEXT,
    session_id TEXT
);

CREATE INDEX idx_conv_msg_created ON conversation_messages(created_at);
CREATE INDEX idx_conv_msg_lane ON conversation_messages(lane_key);
CREATE INDEX idx_conv_msg_lane_id ON conversation_messages(lane_key, id);
CREATE INDEX idx_conv_msg_session ON conversation_messages(session_id, id);
CREATE INDEX idx_conv_msg_task ON conversation_messages(task_id);

CREATE TABLE message_feedback (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL UNIQUE,
    feedback TEXT NOT NULL CHECK(feedback IN ('positive', 'negative')),
    comment TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now')),
    FOREIGN KEY (message_id) REFERENCES conversation_messages(id) ON DELETE CASCADE
);

CREATE INDEX idx_mf_feedback ON message_feedback(feedback);

CREATE TABLE lane_followups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    lane_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('followup','unprocessed_steering')),
    content TEXT NOT NULL,
    principal_json TEXT NOT NULL,
    workspace_path TEXT,
    source_task_id TEXT,
    status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','running','done','cancelled')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    session_id TEXT,
    unattended INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_lane_followups_lane_status ON lane_followups(lane_key, status, id);

-- Memory, full-text search, and 768-dimensional embeddings

CREATE TABLE memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    scope TEXT NOT NULL,
    scope_id TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    importance REAL NOT NULL DEFAULT 0.5,
    confidence REAL NOT NULL DEFAULT 0.7,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    metadata TEXT,
    updated_at TEXT,
    supersedes_id INTEGER REFERENCES memory(id) ON DELETE SET NULL,
    last_accessed_at TEXT
);

CREATE UNIQUE INDEX idx_memory_content_hash ON memory(owner_id, scope, scope_id, content_hash);
CREATE INDEX idx_memory_decay ON memory(owner_id, kind, last_accessed_at);
CREATE INDEX idx_memory_importance ON memory(owner_id, importance);
CREATE INDEX idx_memory_owner ON memory(owner_id);
CREATE INDEX idx_memory_owner_created ON memory(owner_id, created_at DESC);
CREATE INDEX idx_memory_owner_kind ON memory(owner_id, kind);
CREATE INDEX idx_memory_owner_scope ON memory(owner_id, scope, scope_id);
CREATE INDEX idx_memory_supersedes ON memory(supersedes_id);

CREATE VIRTUAL TABLE memory_fts USING fts5 (
    content,
    owner_id UNINDEXED,
    kind UNINDEXED,
    scope UNINDEXED,
    scope_id UNINDEXED,
    source UNINDEXED,
    content='memory',
    content_rowid='id'
);

-- Extension-owned shadow tables are created by vec0, not by this baseline.

CREATE VIRTUAL TABLE memory_vec USING vec0 (
    memory_id INTEGER PRIMARY KEY,
    embedding float[768]
);

-- Keep the external-content FTS index synchronized. Access/decay updates
-- do not reindex content; only the content-update trigger does.

CREATE TRIGGER memory_ai AFTER INSERT ON memory BEGIN
    INSERT INTO memory_fts(rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES (NEW.id, NEW.content, NEW.owner_id, NEW.kind, NEW.scope,
            COALESCE(NEW.scope_id, ''), NEW.source);
END;

CREATE TRIGGER memory_ad AFTER DELETE ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES ('delete', OLD.id, OLD.content, OLD.owner_id, OLD.kind, OLD.scope,
            COALESCE(OLD.scope_id, ''), OLD.source);
END;

CREATE TRIGGER memory_au AFTER UPDATE OF content ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES ('delete', OLD.id, OLD.content, OLD.owner_id, OLD.kind, OLD.scope,
            COALESCE(OLD.scope_id, ''), OLD.source);
    INSERT INTO memory_fts(rowid, content, owner_id, kind, scope, scope_id, source)
    VALUES (NEW.id, NEW.content, NEW.owner_id, NEW.kind, NEW.scope,
            COALESCE(NEW.scope_id, ''), NEW.source);
END;

-- Files and artifact history

-- NULL project_root addresses the home store; NULL rel_path is not unique.

CREATE TABLE file_assets (
    id TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    storage_path TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'uploaded',
    extracted_text TEXT,
    extract_error TEXT,
    metadata_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    origin TEXT NOT NULL DEFAULT 'upload',
    kind TEXT,
    task_id TEXT REFERENCES task(id) ON DELETE SET NULL,
    agent_id TEXT,
    agent_template_id TEXT,
    project_root TEXT,
    rel_path TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    version_count INTEGER NOT NULL DEFAULT 1,
    pinned INTEGER NOT NULL DEFAULT 0,
    summary TEXT,
    missing_since TEXT
);

CREATE UNIQUE INDEX idx_file_assets_addr ON file_assets(COALESCE(project_root, ''), rel_path) WHERE rel_path IS NOT NULL;
CREATE INDEX idx_file_assets_origin ON file_assets(origin, created_at DESC);
CREATE INDEX idx_file_assets_owner ON file_assets(owner_id);
CREATE INDEX idx_file_assets_project ON file_assets(project_root);
CREATE INDEX idx_file_assets_sha256 ON file_assets(sha256);
CREATE INDEX idx_file_assets_task ON file_assets(task_id);

CREATE TABLE artifact_versions (
    artifact_id TEXT NOT NULL REFERENCES file_assets(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    rel_path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    note TEXT,
    author_agent_id TEXT,
    added_lines INTEGER,
    removed_lines INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (artifact_id, version)
);

CREATE INDEX idx_artifact_versions_artifact ON artifact_versions(artifact_id, version DESC);

CREATE TABLE conversation_message_attachments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES file_assets(id) ON DELETE CASCADE,
    sort_order INTEGER NOT NULL DEFAULT 0,
    role TEXT NOT NULL DEFAULT 'attachment',
    caption TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_msg_attach_message ON conversation_message_attachments(message_id);

-- Audit and usage

CREATE TABLE event_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT DEFAULT (datetime('now')),
    agent_id TEXT,
    event_type TEXT NOT NULL,
    detail TEXT,
    result TEXT,
    task_id TEXT
);

CREATE INDEX idx_event_log_agent ON event_log(agent_id);
CREATE INDEX idx_event_log_task ON event_log(task_id);
CREATE INDEX idx_event_log_timestamp ON event_log(timestamp);
CREATE INDEX idx_event_log_type ON event_log(event_type);

CREATE TABLE llm_call_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT DEFAULT (datetime('now')),
    agent_id TEXT,
    task_id TEXT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    key_id TEXT,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cost_usd REAL DEFAULT 0,
    status TEXT DEFAULT 'success',
    latency_ms INTEGER,
    error_message TEXT
);

CREATE INDEX idx_llm_call_log_agent ON llm_call_log(agent_id, timestamp DESC);
CREATE INDEX idx_llm_call_log_task ON llm_call_log(task_id, timestamp DESC);
CREATE INDEX idx_llm_call_log_timestamp ON llm_call_log(timestamp);

CREATE TABLE llm_usage_daily (
    date TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    model TEXT NOT NULL,
    total_requests INTEGER DEFAULT 0,
    total_input_tokens INTEGER DEFAULT 0,
    total_output_tokens INTEGER DEFAULT 0,
    total_cost_usd REAL DEFAULT 0,
    PRIMARY KEY (date, agent_id, model)
);

CREATE TABLE discovered_models (
    model_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    input_price_per_million REAL DEFAULT 0,
    output_price_per_million REAL DEFAULT 0,
    context_window INTEGER DEFAULT 0,
    discovered_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (model_id)
);

CREATE INDEX idx_discovered_models_provider ON discovered_models(provider);

CREATE TABLE orchestrator_latency (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL,
    mode TEXT NOT NULL,
    ack_ms INTEGER DEFAULT 0,
    fallback_reason TEXT,
    auto_promotion_reason TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

CREATE INDEX idx_orch_latency_mode ON orchestrator_latency(mode, timestamp DESC);
CREATE INDEX idx_orch_latency_ts ON orchestrator_latency(timestamp DESC);

CREATE TABLE dispatch_decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL,
    task_id TEXT,
    mode TEXT NOT NULL,
    reason TEXT NOT NULL,
    agent_count INTEGER DEFAULT 0,
    dag_node_count INTEGER,
    predictability_score REAL,
    timestamp TEXT DEFAULT (datetime('now')),
    error_message TEXT
);

CREATE INDEX idx_dd_mode ON dispatch_decisions(mode, timestamp DESC);
CREATE INDEX idx_dd_request ON dispatch_decisions(request_id);
CREATE INDEX idx_dd_ts ON dispatch_decisions(timestamp DESC);

-- Timestamp-leading index names are used explicitly by repository queries.

CREATE TABLE skill_execution_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    agent_id TEXT NOT NULL DEFAULT 'orchestrator',
    status TEXT NOT NULL,
    finish_reason TEXT,
    error_message TEXT,
    validation_failures TEXT,
    duration_ms INTEGER NOT NULL,
    rounds_used INTEGER,
    tool_calls_made INTEGER,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cost_usd REAL DEFAULT 0.0,
    model_used TEXT,
    query_preview TEXT,
    route_score REAL,
    was_auto_selected INTEGER DEFAULT 0,
    repair_attempted INTEGER DEFAULT 0,
    repair_succeeded INTEGER DEFAULT 0,
    timestamp TEXT DEFAULT (datetime('now')),
    response_message_id INTEGER
);

CREATE INDEX idx_sel_agent ON skill_execution_log(agent_id, skill_id);
CREATE UNIQUE INDEX idx_sel_request_id ON skill_execution_log(request_id);
CREATE INDEX idx_sel_response_msg ON skill_execution_log(response_message_id);
CREATE INDEX idx_sel_skill_ts ON skill_execution_log(skill_id, timestamp DESC);
CREATE INDEX idx_sel_status ON skill_execution_log(skill_id, status);
CREATE INDEX idx_sel_timestamp ON skill_execution_log(timestamp, skill_id);

-- Payloads live in the session log; these rows hold previews and references.

CREATE TABLE tool_execution_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT,
    agent_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    success INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    error_message TEXT,
    timestamp TEXT DEFAULT (datetime('now')),
    session_id TEXT,
    task_id TEXT,
    log_seq INTEGER,
    args_preview TEXT,
    result_preview TEXT,
    result_ref TEXT
);

CREATE INDEX idx_tel_request ON tool_execution_log(request_id);
CREATE INDEX idx_tel_session ON tool_execution_log(session_id, id);
CREATE INDEX idx_tel_task ON tool_execution_log(task_id, id);
CREATE INDEX idx_tel_timestamp ON tool_execution_log(timestamp, tool_name);
CREATE INDEX idx_tel_tool_ts ON tool_execution_log(tool_name, timestamp DESC);
