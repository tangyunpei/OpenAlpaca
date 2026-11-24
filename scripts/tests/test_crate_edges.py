#!/usr/bin/env python3.11
"""Tests for `scripts/crate_edges.py`, the workspace's crate-edge check (B-25).

Run with either:
  python3.11 -I scripts/tests/test_crate_edges.py
  python3.11 -m pytest scripts/tests/test_crate_edges.py

None of them runs cargo: each hands the check a canned `cargo metadata --no-deps` document
(`--metadata FILE`) and a small tree holding the table it is checked against, the canned
document and that table agreeing until a test plants one difference. The last two read this
repository's own table: it must list exactly the workspace's members, by the package names
their manifests give, with the three kinds each member's table holds.
"""

import copy
import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import tomllib

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
CHECK = SCRIPTS / "crate_edges.py"

_spec = importlib.util.spec_from_file_location("crate_edges", CHECK)
edges = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(edges)


def _dep(name: str, kind=None, path: bool = True, rename=None) -> dict:
    dep = {"name": name, "kind": kind, "rename": rename, "optional": False, "target": None,
           "req": "*", "source": None if path else "registry+https://github.com/rust-lang/crates.io-index"}
    if path:
        dep["path"] = "/ws/crates/" + name
    return dep


def _pkg(name: str, deps: list[dict]) -> dict:
    return {"name": name, "id": "path+file:///ws/crates/%s#0.1.0" % name, "dependencies": deps}


# Three members: a leaf, a library naming it (normal and dev), and an app naming both, with
# a build edge; outside crates (serde, a registry crate that happens to share no member's
# name) never count.
GOOD_METADATA = {
    "packages": [
        _pkg("leaf", [_dep("serde", path=False)]),
        _pkg("lib", [_dep("leaf"), _dep("leaf", kind="dev"), _dep("tokio", path=False)]),
        _pkg("app", [_dep("leaf"), _dep("lib"), _dep("lib", kind="build"), _dep("tempfile", kind="dev", path=False)]),
    ],
    "workspace_members": [
        "path+file:///ws/crates/leaf#0.1.0",
        "path+file:///ws/crates/lib#0.1.0",
        "path+file:///ws/crates/app#0.1.0",
    ],
    "version": 1,
}

GOOD_TABLE = """\
# The canned workspace's edges.

[app]
dependencies = ["leaf", "lib"]
build-dependencies = ["lib"]
dev-dependencies = []

[leaf]
dependencies = []
build-dependencies = []
dev-dependencies = []

[lib]
dependencies = ["leaf"]
build-dependencies = []
dev-dependencies = ["leaf"]
"""


def _run(metadata, table: str = GOOD_TABLE) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        (root / "scripts").mkdir()
        (root / "scripts" / "crate_edges.toml").write_text(table, encoding="utf-8")
        meta = root / "metadata.json"
        meta.write_text(metadata if isinstance(metadata, str) else json.dumps(metadata), encoding="utf-8")
        proc = subprocess.run(
            [sys.executable, "-I", str(CHECK), "--metadata", str(meta), str(root)],
            capture_output=True, text=True, check=False)
        return proc.returncode, proc.stdout + proc.stderr


def _member(meta: dict, name: str) -> dict:
    return next(p for p in meta["packages"] if p["name"] == name)


def test_the_canned_workspace_matches_its_table() -> None:
    rc, out = _run(GOOD_METADATA)
    assert rc == 0, out
    assert "3 member(s), 5 edge(s), 0 difference(s)" in out, out


def test_an_edge_the_table_lacks_fails_naming_it() -> None:
    meta = copy.deepcopy(GOOD_METADATA)
    _member(meta, "leaf")["dependencies"].append(_dep("lib"))
    rc, out = _run(meta)
    assert rc == 1, out
    assert "edge added: leaf -> lib [dependencies]" in out, out
    assert "1 difference(s)" in out, out


def test_an_edge_the_manifests_dropped_fails_naming_it() -> None:
    meta = copy.deepcopy(GOOD_METADATA)
    _member(meta, "app")["dependencies"] = [d for d in _member(meta, "app")["dependencies"]
                                            if not (d["name"] == "leaf" and d["kind"] is None)]
    rc, out = _run(meta)
    assert rc == 1, out
    assert "edge removed: app -> leaf [dependencies]" in out, out
    assert "1 difference(s)" in out, out


def test_the_kinds_are_kept_apart() -> None:
    # lib's dev edge to leaf becomes a build edge: one edge added, one removed, by kind
    meta = copy.deepcopy(GOOD_METADATA)
    for d in _member(meta, "lib")["dependencies"]:
        if d["kind"] == "dev":
            d["kind"] = "build"
    rc, out = _run(meta)
    assert rc == 1, out
    assert "edge added: lib -> leaf [build-dependencies]" in out, out
    assert "edge removed: lib -> leaf [dev-dependencies]" in out, out
    assert "2 difference(s)" in out, out


def test_a_renamed_dependency_is_its_package() -> None:
    meta = copy.deepcopy(GOOD_METADATA)
    for d in _member(meta, "app")["dependencies"]:
        if d["name"] == "lib" and d["kind"] is None:
            d["rename"] = "the_lib"
    rc, out = _run(meta)
    assert rc == 0, out


def test_a_registry_crate_named_like_a_member_is_no_edge() -> None:
    meta = copy.deepcopy(GOOD_METADATA)
    _member(meta, "leaf")["dependencies"].append(_dep("lib", path=False))
    rc, out = _run(meta)
    assert rc == 0, out


def test_a_member_added_or_dropped_fails_naming_it() -> None:
    meta = copy.deepcopy(GOOD_METADATA)
    meta["packages"].append(_pkg("extra", []))
    meta["workspace_members"].append("path+file:///ws/crates/extra#0.1.0")
    rc, out = _run(meta)
    assert rc == 1 and "member added: extra" in out, out
    rc, out = _run(GOOD_METADATA, GOOD_TABLE + '\n[gone]\ndependencies = []\nbuild-dependencies = []\ndev-dependencies = []\n')
    assert rc == 1 and "member removed: gone" in out, out


def test_a_table_it_cannot_read_is_no_verdict() -> None:
    rc, out = _run(GOOD_METADATA, "[app\n")
    assert rc == 2 and "cannot read" in out, out
    rc, out = _run(GOOD_METADATA, GOOD_TABLE.replace('dev-dependencies = []\n\n[leaf]', '\n[leaf]'))
    assert rc == 2 and "cannot read" in out and "app" in out, out
    rc, out = _run(GOOD_METADATA, GOOD_TABLE.replace('["leaf", "lib"]', '["leaf", "leaf"]'))
    assert rc == 2 and "cannot read" in out, out
    rc, out = _run(GOOD_METADATA, GOOD_TABLE + "\n[stray]\ndependencies = 3\nbuild-dependencies = []\ndev-dependencies = []\n")
    assert rc == 2 and "cannot read" in out, out


def test_metadata_it_cannot_read_is_no_verdict() -> None:
    rc, out = _run("{not json")
    assert rc == 2 and "cannot read" in out, out
    rc, out = _run({"packages": [], "version": 1})
    assert rc == 2 and "cannot read" in out, out


def test_a_usage_error_is_no_verdict() -> None:
    proc = subprocess.run([sys.executable, "-I", str(CHECK), "--bogus"],
                          capture_output=True, text=True, check=False)
    assert proc.returncode == 2, proc.stdout + proc.stderr


def _repo_members() -> dict[str, pathlib.Path]:
    root = SCRIPTS.parent
    ws = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    out = {}
    for member in ws["members"]:
        manifest = root / member / "Cargo.toml"
        out[tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]["name"]] = manifest
    return out


def test_this_repository_table_lists_every_member() -> None:
    table = edges.load_table(SCRIPTS / "crate_edges.toml")
    assert sorted(table) == sorted(_repo_members()), sorted(table)
    for member, kinds in table.items():
        assert sorted(kinds) == sorted(edges.KINDS.values()), (member, sorted(kinds))


def test_this_repository_table_names_only_members() -> None:
    table = edges.load_table(SCRIPTS / "crate_edges.toml")
    members = set(_repo_members())
    for member, kinds in table.items():
        for kind, deps in kinds.items():
            assert deps <= members - {member}, (member, kind, sorted(deps - members))


def main() -> int:
    tests = [(name, fn) for name, fn in sorted(globals().items()) if name.startswith("test_") and callable(fn)]
    failures = 0
    for name, fn in tests:
        try:
            fn()
        except Exception as exc:  # noqa: BLE001 — report every failure, keep going
            failures += 1
            print(f"FAIL {name}: {exc}")
        else:
            print(f"ok   {name}")
    print(f"\n{len(tests) - failures}/{len(tests)} passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
