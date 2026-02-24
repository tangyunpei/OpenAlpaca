# API Docs

> Generated from source by `python3 scripts/gen_api_docs.py`.
> Validate freshness with `python3 scripts/gen_api_docs.py --check`.

## Apps

- [openalpacad](apps/openalpacad.md)
- [openalpaca (CLI)](apps/openalpaca.md)
- [openalpaca-gui](apps/openalpaca-gui.md)

## Crates

- [openalpaca_api](crates/openalpaca_api.md)
- [openalpaca_connectors](crates/openalpaca_connectors.md)
- [openalpaca_core](crates/openalpaca_core.md)
- [openalpaca_llm](crates/openalpaca_llm.md)
- [openalpaca_platform](crates/openalpaca_platform.md)
- [openalpaca_platform_macos](crates/openalpaca_platform_macos.md)
- [openalpaca_storage](crates/openalpaca_storage.md)
- [openalpaca_wake](crates/openalpaca_wake.md)

## Database

- [Schema](database/schema.md)
- [Migrations](database/migrations.md)

## Validation Guarantees

- Route parity: every route in `apps/openalpacad/src/router.rs` is rendered in the daemon API table.
- Migration parity: migration files listed in `crates/openalpaca_storage/src/migrations/mod.rs` match generated migration docs.
- Source link integrity: generated source file references are verified to exist.
