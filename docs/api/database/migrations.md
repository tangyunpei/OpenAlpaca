# Database Migrations

> Generated from migration registry in `crates/openalpaca_storage/src/migrations/mod.rs`.

## Overview

- Total registered migrations: 2
- Migration SQL directory: `crates/openalpaca_storage/src/migrations`

## Files

| Version | Name | SQL File | Summary |
|---|---|---|---|
| 1 | `baseline` | `001_baseline.sql` | Baseline schema, version 1: the whole schema in one pass (the unreleased 001–045 development history squashed on 2026-09-28). A database from any earlier schema line is refused, never upgraded. |
| 2 | `working_state` | `002_working_state.sql` | Working state, version 2: `session`'s four summary columns become the conversation's working state — the state's JSON, its version, the last row it covers, when it was written, and the last conversation summary projected from it (design 11 §2, §6; owner decision CQ-9). |
