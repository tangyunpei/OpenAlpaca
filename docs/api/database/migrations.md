# Database Migrations

> Generated from migration registry in `crates/openalpaca_storage/src/migrations/mod.rs`.

## Overview

- Total registered migrations: 35
- Migration SQL directory: `crates/openalpaca_storage/src/migrations`

## Files

| Version | Name | SQL File | Summary |
|---|---|---|---|
| 1 | `init` | `001_init.sql` | Schema version tracking |
| 2 | `memory_fts` | `002_memory_fts.sql` | FTS5 全文索引 (幂等版本) |
| 3 | `identity` | `003_identity.sql` | Migration 003: Identity System |
| 4 | `config` | `004_config.sql` | System Configuration Table |
| 5 | `preference` | `005_preference.sql` | Migration 005: User Preference Table |
| 6 | `tasks` | `006_tasks.sql` | Migration 006: Task System |
| 7 | `subagents` | `007_subagents.sql` | Migration 007: SubAgent System |
| 8 | `llm_usage` | `008_llm_usage.sql` | Migration 008: LLM Usage Tracking |
| 9 | `conversation_messages` | `009_conversation_messages.sql` | (no summary comment) |
| 10 | `discovered_models` | `010_discovered_models.sql` | Migration 010: Discovered Models Cache |
| 11 | `unified_conversations` | `011_unified_conversations.sql` | Phase 5.6: Unified Conversation Pipeline |
| 12 | `assignment_output` | `012_assignment_output.sql` | Migration 012: Add result_output to task_agent_assignment |
| 13 | `vec_search` | `013_vec_search.sql` | Migration 013: Vector search support via sqlite-vec |
| 14 | `conversation_summary` | `014_conversation_summary.sql` | (no summary comment) |
| 15 | `memory_v2` | `015_memory_v2.sql` | Drop old triggers, FTS, vec, and base table |
| 16 | `task_state` | `016_task_state.sql` | (no summary comment) |
| 17 | `vec_768` | `017_vec_768.sql` | Migration 017: Upgrade memory_vec from 384-dim to 768-dim embeddings |
| 18 | `memory_lifecycle` | `018_memory_lifecycle.sql` | Migration 018: Memory lifecycle — supersession + importance decay |
| 19 | `memory_fixes` | `019_memory_fixes.sql` | Migration 019: Performance index for recent() query |
| 20 | `agent_template_id` | `020_agent_template_id.sql` | Migration 020: Add template_id to agent table |
| 21 | `agent_template_id_not_null` | `021_agent_template_id_not_null.sql` | Migration 021: Enforce template_id NOT NULL on agent table |
| 22 | `orchestrator_latency` | `022_orchestrator_latency.sql` | Migration 022: Orchestrator latency metrics for request-level observability |
| 23 | `dispatch_decision` | `023_dispatch_decision.sql` | Migration 023: Dispatch decision history for orchestrator analysis |
| 24 | `dispatch_decision_request_id` | `024_dispatch_decision_request_id.sql` | Migration 024: Rename task_id -> request_id, add task_id as nullable backfill |
| 25 | `dispatch_decision_error_message` | `025_dispatch_decision_error_message.sql` | Migration 025: Add error_message column to dispatch_decisions |
| 26 | `memory_scope_dedup` | `026_memory_scope_dedup.sql` | Replace owner-only dedup with scope-aware dedup. |
| 27 | `file_assets` | `027_file_assets.sql` | File assets storage for multimodal chat |
| 28 | `message_attachments` | `028_message_attachments.sql` | Message attachments linking messages to file assets |
| 29 | `task_outcome` | `029_task_outcome.sql` | Migration 029: Task outcome fields |
| 30 | `skill_tool_execution_log` | `030_skill_tool_execution_log.sql` | (no summary comment) |
| 31 | `message_feedback` | `031_message_feedback.sql` | Phase 4b: User Feedback Signal |
| 32 | `context_compaction_log` | `032_context_compaction_log.sql` | Context compaction telemetry (Phase B) |
| 33 | `lane_followups` | `033_lane_followups.sql` | Lane follow-up queue (Routing V2): explicit `queue_followup` items and |
| 34 | `drop_context_compaction_log` | `034_drop_context_compaction_log.sql` | Drop context_compaction_log: the table was added in migration 032 but no |
| 35 | `drop_planner_telemetry` | `035_drop_planner_telemetry.sql` | Migration 035: drop the planner telemetry columns. |
