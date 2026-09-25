# Database Migrations

> Generated from migration registry in `crates/openalpaca_storage/src/migrations/mod.rs`.

## Overview

- Total registered migrations: 3
- Migration SQL directory: `crates/openalpaca_storage/src/migrations`

## Files

| Version | Name | SQL File | Summary |
|---|---|---|---|
| 43 | `baseline` | `001_baseline.sql` | Baseline schema: the final state of the unreleased 001-042 migration history, re-squashed as version 43 without the SQL memory engine. |
| 44 | `learning` | `044_learning.sql` | 044: Learning v2 (tasks/memory-redesign/09): the turn id and project root on each message and a per-session learning cursor. |
| 45 | `episodes` | `045_episodes.sql` | 045: Conversation summaries (tasks/memory-redesign/09 §13.3): a per-session episode cursor beside the learning cursor. |
