# Database Migrations

> Generated from migration registry in `crates/openalpaca_storage/src/migrations/mod.rs`.

## Overview

- Total registered migrations: 1
- Migration SQL directory: `crates/openalpaca_storage/src/migrations`
- The database runner records each applied version in `schema_migrations` in the same transaction as its SQL.
- Append the next numbered migration to the registry; SQL files do not insert version rows.

## Files

| Version | Name | SQL File | Summary |
|---|---|---|---|
| 1 | `baseline` | `001_baseline.sql` | Initial application schema, version 1. |
