> Evidence appendix to [05-review.md](05-review.md), rev 2 review of 2026-09-23. An isolated spike, run outside the repository, that answers the open question in [01-code-audit.md](01-code-audit.md) ("Schema retirement trap") and [04-implementation-plan.md](04-implementation-plan.md) rev 1 §4: can a database holding the `memory_vec` vec0 table be retired by a build that no longer links sqlite-vec?

# Spike: retiring `memory_vec` without sqlite-vec

## Setup

- A throwaway cargo project outside the repository, with `rusqlite = "=0.38.0"` (features `bundled`, `vtab`) and `sqlite-vec = "=0.1.9"`. These are the versions in the workspace `Cargo.lock`, and the bundled SQLite is 3.51.1. It built offline from the local registry cache.
- The memory DDL is copied verbatim from `crates/openalpaca_storage/src/migrations/001_baseline.sql` (the `memory`, `memory_fts`, three triggers and `memory_vec float[768]` block, baseline lines 272–341).
- sqlite-vec is registered the way `crates/openalpaca_storage/src/database/mod.rs` does it: `sqlite3_auto_extension`, with the same PRAGMAs.
- The databases were created with the real module and filled with rows and three vectors. Each question then ran in a **separate process**, so no module registration carried over.
- Only synthetic databases in the scratch directory were used. No repository file and no `~/.openalpaca` data was touched.

## Results

| Question | Result |
|---|---|
| **Q1**: without the module, can `DROP TABLE memory_vec` run? | **No.** It fails with `no such module: vec0` (SQLITE_ERROR). `DROP TABLE IF EXISTS`, `DELETE FROM memory_vec` and even *preparing* `SELECT … FROM memory_vec` fail the same way. **Everything else works:** preparing, reading and writing other tables, the FTS triggers and `MATCH`, `ALTER TABLE` add/rename, `CREATE`/`DROP TABLE`, `VACUUM`, and reading the vec0 shadow tables. `PRAGMA integrity_check` and `quick_check` both return `ok`. An unknown module matters only to a statement that names the vec0 table. |
| **Q2**: drop it with the module, then reopen without it? | **Clean.** vec0's `xDestroy` removes its four shadow tables (`memory_vec_chunks`, `memory_vec_info`, `memory_vec_rowids`, `memory_vec_vector_chunks00`), their two autoindexes and their two `sqlite_sequence` rows. Reopened without the module, nothing is left and the integrity checks return `ok`. |
| **Q3**: is there a supported way to drop it **without** sqlite-vec, and without `PRAGMA writable_schema`? | **Yes: a stub `vec0` module.** It uses rusqlite's `vtab` feature, which adds no dependencies. `xConnect` declares any schema, `xCreate` refuses, and `xDestroy` is a no-op. Dropping `memory_vec` with the stub leaves the four shadow tables as ordinary tables, which plain `DROP TABLE` then removes. The integrity checks return `ok`, including in a fresh process with nothing registered. A rolled-back attempt leaves the vec0 table intact, and the real sqlite-vec still reads all three vectors. **Hazard:** while the stub is registered, `SELECT COUNT(*) FROM memory_vec` silently returns **0**. So register it only on the migration connection, only for the retirement statements, and unregister it afterwards (`sqlite3_drop_modules(db, NULL)`). |
| **Q4**: can the FTS side be dropped without sqlite-vec? | **Yes, with no issue.** Dropping the three triggers, then `memory_fts` (its four FTS5 shadow tables go with it), then `memory` works while the unreadable vec0 table is still present. The integrity checks return `ok`. Dropping `memory` first also works: its triggers go with it, `memory_fts` becomes unreadable but still drops. |
| **Fresh database** | The **current baseline cannot run without sqlite-vec**. Creating `memory_vec` fails with `no such module: vec0`, and the stub cannot help because it refuses to create. The baseline itself must stop creating `memory_vec`. |

## Retirement sequence the evidence supports

Tested inside a transaction, the way `run_migrations` runs a migration. It gave the same clean result on three database states:
- `memory_vec` still present
- already retired
- `memory_vec` already dropped by the real module

1. Register the stub `vec0` module on the connection that runs the migrations.
2. Run, in **one** transaction:
   ```sql
   DROP TABLE IF EXISTS memory_vec;
   DROP TABLE IF EXISTS memory_vec_chunks;
   DROP TABLE IF EXISTS memory_vec_info;
   DROP TABLE IF EXISTS memory_vec_rowids;
   DROP TABLE IF EXISTS memory_vec_vector_chunks00;
   -- then the memory triggers, memory_fts and memory, as the redesign requires
   INSERT INTO schema_version(version) VALUES (43);
   ```
   Without the stub, this fails and rolls back, and the database is unchanged.
3. Unregister the stub.
4. Remove `memory_vec`, and the other memory tables, from `001_baseline.sql`, so a fresh database never needs the module.

## The stub module (core of the spike code)

```rust
use rusqlite::vtab::{read_only_module, Context, CreateVTab, Filters, IndexInfo,
                     VTab, VTabConnection, VTabCursor, VTabKind};
use rusqlite::{ffi, Connection, Result};

#[repr(C)]
struct StubVec0 { base: ffi::sqlite3_vtab }

unsafe impl<'vtab> VTab<'vtab> for StubVec0 {
    type Aux = ();
    type Cursor = StubCursor<'vtab>;
    fn connect(_db: &mut VTabConnection, _aux: Option<&()>, _args: &[&[u8]])
        -> Result<(String, Self)> {
        Ok(("CREATE TABLE x(memory_id INTEGER, embedding BLOB)".to_owned(),
            StubVec0 { base: ffi::sqlite3_vtab::default() }))
    }
    fn best_index(&self, info: &mut IndexInfo) -> Result<()> {
        info.set_estimated_cost(1e12); Ok(())
    }
    fn open(&'vtab mut self) -> Result<StubCursor<'vtab>> { /* empty cursor */ }
}

impl<'vtab> CreateVTab<'vtab> for StubVec0 {
    const KIND: VTabKind = VTabKind::Default;
    fn create(_db: &mut VTabConnection, _aux: Option<&()>, _args: &[&[u8]])
        -> Result<(String, Self)> {
        Err(rusqlite::Error::ModuleError("stub vec0 cannot create tables".into()))
    }
    fn destroy(&self) -> Result<()> { Ok(()) }   // shadow tables are dropped by plain SQL
}
// StubCursor: eof() == true; every other method is a no-op.

fn register_stub(conn: &Connection) -> Result<()> {
    conn.create_module(c"vec0", read_only_module::<StubVec0>(), None)
}
```

## Limits

The spike used synthetic rows. It did not open a schema-42 database produced by the real daemon. [04-implementation-plan.md](04-implementation-plan.md) rev 2 therefore makes a committed schema-42 fixture database, generated by the current build **while sqlite-vec is still linked**, the first acceptance test for migration 043. The spike ran under the bundled SQLite 3.51.1 only, and the retirement must be re-tested if the `rusqlite` or SQLite version changes before the migration lands.
