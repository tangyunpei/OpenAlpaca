#!/usr/bin/env python3.11
"""Regression tests for the `scripts/gen_api_docs.py` parsers.

Run with either:
  python3.11 scripts/tests/test_gen_api_docs.py
  python3.11 -m pytest scripts/tests/test_gen_api_docs.py

Every case here is a shape that silently lost content before the parsers learned
to mask comments and literals: the generator never raised, the docs simply came
out short. Each test therefore asserts the content is present, not that some
exception is absent.
"""

import contextlib
import importlib.util
import io
import pathlib
import sys
import tempfile


SCRIPTS = pathlib.Path(__file__).resolve().parents[1]


def _load_generator():
    spec = importlib.util.spec_from_file_location("gen_api_docs", SCRIPTS / "gen_api_docs.py")
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules["gen_api_docs"] = module
    spec.loader.exec_module(module)
    return module


gen = _load_generator()
FAKE = pathlib.Path("apps/openalpacad/src/routes/fake.rs")


def _fields(type_defs, qualified: str) -> list[str]:
    assert qualified in type_defs, f"{qualified} was dropped; found {sorted(type_defs)}"
    return [name for name, _type in type_defs[qualified].fields]


# ── mask_rust_noncode ────────────────────────────────────────────────────


def test_mask_keeps_offsets_and_blanks_only_prose() -> None:
    source = '/// the agent\'s id\npub struct A { pub x: String } // tail\n'
    masked = gen.mask_rust_noncode(source)
    assert len(masked) == len(source)
    assert masked.count("\n") == source.count("\n")
    assert "pub struct A { pub x: String }" in masked
    assert "agent" not in masked
    assert "tail" not in masked


def test_mask_leaves_lifetimes_and_eats_comment_braces() -> None:
    masked = gen.mask_rust_noncode("struct D<'a> { /* } */ pub db: &'a Database }")
    assert masked.count("{") == 1
    assert masked.count("}") == 1
    assert "&'a Database" in masked


def test_mask_does_not_read_a_url_in_a_literal_as_a_comment() -> None:
    masked = gen.mask_rust_noncode('let url = "http://x//y"; let keep = 1;')
    assert "let keep = 1;" in masked


def test_mask_literals_false_keeps_string_contents() -> None:
    masked = gen.mask_rust_noncode('// drop\n#[arg(long = "dry-run")]', mask_literals=False)
    assert 'long = "dry-run"' in masked
    assert "drop" not in masked


# ── case 1/2: an apostrophe, and an odd count of them, in a struct body ──


def test_apostrophe_in_a_field_doc_does_not_glue_on_the_next_struct() -> None:
    source = """
#[derive(Serialize)]
pub struct PatchRequest {
    pub title: Option<String>,
    /// A path binds the session's project; absent leaves it alone.
    pub workspace_path: Option<String>,
}

#[derive(Serialize)]
pub struct MessagesResponse {
    /// The page's rows.
    pub messages: Vec<View>,
    pub total: i64,
}
"""
    defs = gen.parse_type_defs(source, FAKE, "fake")
    assert _fields(defs, "fake::PatchRequest") == ["title", "workspace_path"]
    assert _fields(defs, "fake::MessagesResponse") == ["messages", "total"]


def test_odd_apostrophe_count_no_longer_drops_the_struct_that_follows() -> None:
    source = """
pub struct RetentionStatus {
    /// The denominator `sessions.last_sweep`'s `over_cap_after` is measured against.
    pub log_max_total_bytes: u64,
}

/// The session-log numbers, from the service the runner already holds.
pub struct SessionsStatus {
    /// The boot sweep's account, or `null` when no pass ran.
    pub last_sweep: Option<SweepStatus>,
    pub dropped_records: u64,
}
"""
    defs = gen.parse_type_defs(source, FAKE, "fake")
    assert _fields(defs, "fake::RetentionStatus") == ["log_max_total_bytes"]
    assert _fields(defs, "fake::SessionsStatus") == ["last_sweep", "dropped_records"]


def test_a_struct_named_inside_a_doc_comment_is_not_a_struct() -> None:
    source = """
/// Mirrors `pub struct Ghost { pub gone: bool }` on the client.
pub struct Real {
    pub here: bool,
}
"""
    defs = gen.parse_type_defs(source, FAKE, "fake")
    assert sorted(defs) == ["fake::Real"]


# ── case 3: a comma in a clap doc comment ───────────────────────────────


def test_a_comma_in_variant_help_does_not_invent_a_variant() -> None:
    source = """
#[derive(Subcommand)]
pub enum TasksCommands {
    /// Resume a run: un-pause a paused one, and continue an interrupted one.
    Resume {
        task_id: String,
    },
    /// Cancel a run
    Cancel {
        task_id: String,
    },
}
"""
    enums = gen.parse_subcommand_enums(source)
    assert [e["name"] for e in enums] == ["TasksCommands"]
    assert [v["command"] for v in enums[0]["variants"]] == ["resume", "cancel"]
    assert [v["fields"] for v in enums[0]["variants"]] == [["task_id"], ["task_id"]]


# ── case 4: `//` and apostrophes inside a Subcommand enum body ──────────


def test_a_plain_comment_line_does_not_shadow_the_variant_after_it() -> None:
    source = """
#[derive(Subcommand)]
pub enum StoreCommands {
    // kept for the 0.4 flag spelling, removed once the GUI stops sending it
    Rebase {
        old: String,
        new: String,
    },
}
"""
    enums = gen.parse_subcommand_enums(source)
    assert [v["command"] for v in enums[0]["variants"]] == ["rebase"]
    assert enums[0]["variants"][0]["fields"] == ["old", "new"]


def test_an_apostrophe_in_variant_help_keeps_every_variant() -> None:
    source = """
#[derive(Subcommand)]
pub enum ExtCommands {
    /// Show one extension's state
    Info {
        kind: String,
        id: String,
    },
    /// Drop the server's connection and reconnect
    Reload {
        kind: String,
        id: String,
    },
    /// List them
    List {
        format: OutputFormat,
    },
}
"""
    enums = gen.parse_subcommand_enums(source)
    assert [v["command"] for v in enums[0]["variants"]] == ["info", "reload", "list"]


def test_a_doc_comment_between_the_derive_and_the_enum_is_fine() -> None:
    source = """
#[derive(Subcommand)]
/// Written above the enum rather than above the derive.
pub enum GuiAction {
    Start,
    Stop,
}
"""
    enums = gen.parse_subcommand_enums(source)
    assert [v["command"] for v in enums[0]["variants"]] == ["start", "stop"]


# ── case 5: the GUI endpoints table ─────────────────────────────────────


def test_gui_endpoints_read_the_backticked_convention() -> None:
    source = """
/**
 * `/v1/workspaces` — the project a store belongs to.
 */

import { apiFetch } from "../http";

/** `GET /v1/workspaces?path=` — 400 for a relative or missing path. */
export async function getWorkspace(path: string): Promise<Workspace> {
  return await apiFetch("/v1/workspaces", { query: { path } });
}

/**
 * `PATCH /v1/workspaces` — the one transaction.
 */
export async function rebaseWorkspace(a: string): Promise<WorkspaceRebase> {
  return await apiFetch("/v1/workspaces", { method: "PATCH" });
}
"""
    assert gen.parse_gui_endpoints(source) == [
        ("GET", "/v1/workspaces"),
        ("PATCH", "/v1/workspaces"),
    ]


def test_gui_endpoint_falls_back_to_the_module_header() -> None:
    source = """
/**
 * `GET /v1/tools` — the tool catalog.
 */

import { apiFetch } from "../http";

/** Bare array, sorted by name. */
export async function listTools(): Promise<ToolCatalogEntry[]> {
  return await apiFetch("/v1/tools");
}
"""
    assert gen.parse_gui_endpoints(source) == [("GET", "/v1/tools")]


def test_gui_wrapper_over_another_wrapper_claims_no_route() -> None:
    source = """
/** `/v1/events/history` and `/v1/health`. */

import { apiFetch } from "../http";

/** `GET /v1/events/history` — server default 100, clamped at 1000. */
export async function getEventHistory(): Promise<EventHistoryPage> {
  return await apiFetch("/v1/events/history");
}

/**
 * One run's event log (GAP-10, closed). Built on `getEventHistory`.
 */
export async function getRunEventLog(taskId: string): Promise<RunEventPage> {
  return await getEventHistory();
}
"""
    assert gen.parse_gui_endpoints(source) == [("GET", "/v1/events/history")]


# ── the SQL splitter keeps its own quote handling ───────────────────────


def test_sql_column_split_still_respects_quoted_defaults() -> None:
    columns = gen.split_top_level("id TEXT PRIMARY KEY, state TEXT DEFAULT 'a, b', n INTEGER")
    assert columns == ["id TEXT PRIMARY KEY", "state TEXT DEFAULT 'a, b'", "n INTEGER"]


# ── the schema state replays what a migration undid ─────────────────────
#
# `parse_schema_state` is a replay of every migration in order, and a statement
# it does not recognize is a silent no-op: the doc then shows a column or a
# table the real database dropped, with no sign anything was missed.


def _parse_migrations(files: list[tuple[str, str]]):
    """Run `parse_schema_state` over temp .sql files instead of the real ones."""
    with tempfile.TemporaryDirectory() as tmp:
        directory = pathlib.Path(tmp)
        migrations = []
        for index, (file_name, sql) in enumerate(files, start=1):
            (directory / file_name).write_text(sql, encoding="utf-8")
            migrations.append(
                gen.MigrationItem(version=index, name=file_name, file_name=file_name, summary="")
            )
        original = gen.MIGRATIONS_DIR
        gen.MIGRATIONS_DIR = directory
        try:
            return gen.parse_schema_state(migrations)
        finally:
            gen.MIGRATIONS_DIR = original


def test_parse_schema_state_applies_drop_column() -> None:
    tables, _indexes, _triggers = _parse_migrations(
        [
            (
                "001_create.sql",
                "CREATE TABLE latency (\n"
                "    id INTEGER PRIMARY KEY,\n"
                "    planner_ms INTEGER DEFAULT 0,\n"
                "    total_ms INTEGER DEFAULT 0\n"
                ");\n",
            ),
            ("002_drop_column.sql", "ALTER TABLE latency DROP COLUMN planner_ms;\n"),
        ]
    )

    columns = tables["latency"].columns
    assert not any(c.split()[0] == "planner_ms" for c in columns), columns
    assert [c.split()[0] for c in columns] == ["id", "total_ms"], columns
    # The doc attributes a table to the last migration that changed it.
    assert tables["latency"].source_file == "002_drop_column.sql"


def test_parse_schema_state_applies_a_drop_table_without_if_exists() -> None:
    tables, indexes, _triggers = _parse_migrations(
        [
            (
                "001_create.sql",
                "CREATE TABLE gone (id TEXT PRIMARY KEY, note TEXT);\n"
                "CREATE INDEX idx_gone_note ON gone(note);\n"
                "CREATE TABLE kept (id TEXT PRIMARY KEY);\n"
                "CREATE INDEX idx_kept ON kept(id);\n",
            ),
            # SQLite drops a table's indexes with the table, `IF EXISTS` or not.
            ("002_drop_table.sql", "DROP TABLE gone;\n"),
        ]
    )

    assert "gone" not in tables, sorted(tables)
    assert "idx_gone_note" not in indexes, sorted(indexes)
    assert "kept" in tables and "idx_kept" in indexes


# ── the workspace's crates, read by tomllib ─────────────────────────────
#
# `parse_workspace_crates` reads `[workspace] members` by its key. The regex it
# replaced took the first `members = [` it met, which would have been
# `default-members` had that come first; and a Cargo.toml that is not TOML is one
# line and exit 2, not a traceback.


def _with_cargo_toml(manifest: str, call):
    """Run `call(path)` with `CARGO_TOML` pointed at a temp Cargo.toml holding `manifest`."""
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "Cargo.toml"
        path.write_text(manifest, encoding="utf-8")
        original = gen.CARGO_TOML
        gen.CARGO_TOML = path
        try:
            return call(path)
        finally:
            gen.CARGO_TOML = original


def test_workspace_crates_are_the_members_not_the_default_members() -> None:
    manifest = (
        "[workspace]\n"
        'default-members = ["crates/b"]\n'
        'members = [\n    "crates/b",\n    "crates/a",  # a comment\n    "apps/x",\n]\n'
    )
    crates = _with_cargo_toml(manifest, lambda _path: gen.parse_workspace_crates())
    assert [p.relative_to(gen.ROOT).as_posix() for p in crates] == ["crates/a", "crates/b"], crates


def test_a_cargo_toml_with_no_workspace_members_has_no_crates() -> None:
    crates = _with_cargo_toml('[package]\nname = "solo"\n', lambda _path: gen.parse_workspace_crates())
    assert crates == [], crates


def test_a_cargo_toml_that_is_not_toml_is_a_source_error_naming_it() -> None:
    def call(path):
        try:
            gen.parse_workspace_crates()
        except gen.SourceError as e:
            return path, str(e)
        raise AssertionError("a Cargo.toml that is not TOML was read")

    path, message = _with_cargo_toml('[workspace\nmembers = ["crates/a"]\n', call)
    assert message.startswith(f"{path} is not TOML: "), message


def test_a_workspace_or_members_of_the_wrong_shape_is_a_source_error_naming_it() -> None:
    def call(path):
        try:
            gen.parse_workspace_crates()
        except gen.SourceError as e:
            return path, str(e)
        raise AssertionError("a Cargo.toml of the wrong shape was read")

    for manifest, what in (
        ("workspace = 1\n", "[workspace] is not a table"),
        ('[workspace]\nmembers = "crates/a"\n', "[workspace] members is not an array of strings"),
        ("[workspace]\nmembers = [1]\n", "[workspace] members is not an array of strings"),
    ):
        path, message = _with_cargo_toml(manifest, call)
        assert message == f"{path} is not a workspace manifest: {what}", (manifest, message)


def test_main_on_a_cargo_toml_that_is_not_toml_exits_2_with_one_line() -> None:
    def call(path):
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = gen.main(["--check"])
        return path, rc, err.getvalue()

    path, rc, err = _with_cargo_toml("[workspace\n", call)
    assert rc == 2, (rc, err)
    lines = err.splitlines()
    assert len(lines) == 1 and lines[0].startswith(f"gen_api_docs: {path} is not TOML: "), lines


def main() -> int:
    tests = [(name, fn) for name, fn in sorted(globals().items()) if name.startswith("test_")]
    failures = 0
    for name, fn in tests:
        try:
            fn()
        except AssertionError as exc:
            failures += 1
            print(f"FAIL {name}: {exc}")
        else:
            print(f"ok   {name}")
    print(f"\n{len(tests) - failures}/{len(tests)} passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
