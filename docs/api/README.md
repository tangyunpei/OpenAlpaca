# API Docs

Reference documentation for the OpenAlpaca daemon HTTP API, CLI, GUI bridge,
workspace crates, and SQLite database. The detailed documents are produced by a
generator script rather than written by hand:

```bash
python3 scripts/gen_api_docs.py          # (re)generate everything under docs/api/
python3 scripts/gen_api_docs.py --check  # exit non-zero if any generated doc is stale
```

Note: this index (`docs/api/README.md`) is maintained by hand and is a superset
of what the generator emits — `gen_api_docs.py` writes its own shorter, plain
link-list index to the same path, so **running the generator overwrites this
file**. Regenerate into a scratch copy, or restore this index afterward, if you
want to keep the extra prose below.

## Generated documents

Only this index is committed to the repository. The detailed documents below
are written into subdirectories of `docs/api/` when you run the generator —
they are not checked in (see [Committing generated output](#committing-generated-output)),
so run `python3 scripts/gen_api_docs.py` locally to produce them.

### Apps

| Output | Contents |
|---|---|
| `apps/openalpacad.md` | Daemon HTTP API: endpoint table parsed from `apps/openalpacad/src/router.rs`, request/query types, streaming notes |
| `apps/openalpaca.md` | CLI: endpoints used, request/response shapes, command source map |
| `apps/openalpaca-gui.md` | Tauri + Svelte GUI: Tauri commands and API module map |

### Crates

One `crates/<name>.md` per workspace crate:

- `openalpaca_api`
- `openalpaca_connectors`
- `openalpaca_core`
- `openalpaca_llm`
- `openalpaca_mcp`
- `openalpaca_platform`
- `openalpaca_platform_macos`
- `openalpaca_plugins`
- `openalpaca_storage`
- `openalpaca_wake`

### Database

| Output | Contents |
|---|---|
| `database/schema.md` | SQLite tables, indexes, and triggers, reconstructed by replaying the migrations |
| `database/migrations.md` | The 32 numbered SQL migrations in `crates/openalpaca_storage/src/migrations/` |

## Committing generated output

The repository's `.gitignore` ignores `*.md` globally and re-includes only the
direct children of this directory (`!docs/api/*`). Generated files under
`docs/api/apps/`, `docs/api/crates/`, and `docs/api/database/` therefore remain
git-ignored: running the generator produces them locally, but they cannot be
committed unless the `.gitignore` rule is widened (e.g. to `!docs/api/**`).

## Validation guarantees

These are assertions the generator runs each time it executes; generation fails
if any of them is violated:

- Route parity: the daemon endpoint table must contain exactly one row per
  route parsed from `apps/openalpacad/src/router.rs` (and the parse must not be
  empty).
- Migration parity: the migrations registered in
  `crates/openalpaca_storage/src/migrations/mod.rs` must match the numbered
  `.sql` files on disk, one to one.
- Source link integrity: every source file referenced by the generated docs
  must exist in the repository.

Because the assertions run at generation time, they hold for freshly generated
output. `--check` compares every path the generator emits against the working
tree; because this hand-maintained index intentionally differs from the
generator's plain output, `--check` will report `docs/api/README.md` itself as
changed — that difference is expected, not a staleness bug in the detailed docs.
