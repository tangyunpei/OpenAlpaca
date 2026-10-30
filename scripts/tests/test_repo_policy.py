#!/usr/bin/env python3
"""Tests for `scripts/repo_policy.py`, the repository-policy guard (B-23).

Run with either:
  python3 -I scripts/tests/test_repo_policy.py
  python3 -m pytest scripts/tests/test_repo_policy.py

Each planted-violation test starts from a small tree the guard passes, plants one
violation, and asserts the guard fails on it and names its rule; the last test runs the
guard on this repository, which must pass. The planted toolchain literal and the
planted unpaired target directory are spelled in two pieces, so that this file — a gate
script the guard reads — is no violation itself.
"""

from __future__ import annotations

import importlib.util
import pathlib
import subprocess
import sys
import tempfile

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
GUARD = SCRIPTS / "repo_policy.py"

CHECKOUT = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1"
GOOD_WORKFLOW = """\
name: CI
on:
  pull_request:
permissions:
  contents: read
jobs:
  rust:
    runs-on: ubuntu-24.04
    env:
      CARGO_TARGET_DIR: /tmp/t
      CARGO_BUILD_BUILD_DIR: /tmp/t
    steps:
      - uses: {checkout}
        with:
          persist-credentials: false
      - name: Build
        run: cargo build --locked --workspace --exclude openalpaca_gui
      - name: Clippy
        run: |
          cargo clippy --locked --workspace \\
            --exclude openalpaca_gui --all-targets
""".format(checkout=CHECKOUT)

GOOD_SCRIPT = """\
#!/bin/sh
# A comment may say cargo {plus}1.99.0 and CARGO_TARGET_DIR=x without either rule firing.
TOOLCHAIN="1.99.0"
CARGO_TARGET_DIR="$T" CARGO_BUILD_BUILD_DIR="$T" cargo "+$TOOLCHAIN" check
export CARGO_TARGET_DIR="$T"
export CARGO_BUILD_BUILD_DIR="$T"
""".format(plus="+")

LOCK_WITH_ZIP8 = """\
version = 4

[[package]]
name = "zip"
version = "8.6.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "zip"
version = "9.0.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
"""

DENY = """\
[advisories]
yanked = "deny"
ignore = [
    { id = "RUSTSEC-2026-0367", reason = "calamine's zip 8.6.0" },
    { id = "RUSTSEC-2026-0368", reason = "calamine's zip 8.6.0" },
    { id = "RUSTSEC-2026-0369", reason = "calamine's zip 8.6.0" },
]
"""


def _load_guard():
    spec = importlib.util.spec_from_file_location("repo_policy", GUARD)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


guard = _load_guard()


def _tree(root: pathlib.Path, workflow: str = GOOD_WORKFLOW, script: str = GOOD_SCRIPT,
          lock: str = LOCK_WITH_ZIP8, deny: str = DENY) -> pathlib.Path:
    (root / ".github" / "workflows").mkdir(parents=True)
    (root / ".github" / "workflows" / "ci.yml").write_text(workflow, encoding="utf-8")
    (root / "scripts").mkdir()
    (root / "scripts" / "gate.sh").write_text(script, encoding="utf-8")
    (root / "Cargo.lock").write_text(lock, encoding="utf-8")
    (root / "deny.toml").write_text(deny, encoding="utf-8")
    return root


def _run(root: pathlib.Path) -> tuple[int, str]:
    proc = subprocess.run([sys.executable, "-I", str(GUARD), str(root)], capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def _planted(rule: str, **kw) -> None:
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(_tree(pathlib.Path(d), **kw))
        assert rc == 1, "rc=%d, expected the guard to fail on a planted %s violation:\n%s" % (rc, rule, out)
        assert (": %s: " % rule) in out, "the failure does not name %s:\n%s" % (rule, out)
        assert out.count(": %s: " % rule) == 1, "one violation planted, more reported:\n%s" % out


def test_the_fixture_tree_passes() -> None:
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(_tree(pathlib.Path(d)))
        assert rc == 0, out
        assert "0 violation(s)" in out, out


def test_an_unlocked_cargo_call_fails() -> None:
    _planted("locked", workflow=GOOD_WORKFLOW.replace("cargo build --locked", "cargo build"))


def test_an_unlocked_cargo_call_on_a_continued_line_fails() -> None:
    _planted("locked", workflow=GOOD_WORKFLOW.replace("cargo clippy --locked --workspace", "cargo clippy --workspace"))


def test_an_unlocked_cargo_call_after_a_global_option_fails() -> None:
    # cargo's global options come before the subcommand, a value-taking one with its value
    for call in ("cargo -q build", "cargo --color never build", "cargo --config build.jobs=4 build"):
        _planted("locked", workflow=GOOD_WORKFLOW.replace("cargo build --locked", call))
    # an option's value that begins with a subcommand's name is not the subcommand
    quiet = GOOD_WORKFLOW + "      - run: cargo --config test.jobs=4 metadata --format-version 1\n"
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(_tree(pathlib.Path(d), workflow=quiet))
        assert rc == 0, out


def test_a_tag_pinned_uses_fails() -> None:
    _planted("sha-pin", workflow=GOOD_WORKFLOW.replace(CHECKOUT, "actions/checkout@v7"))


def test_a_sha_pin_without_its_version_comment_fails() -> None:
    _planted("sha-pin", workflow=GOOD_WORKFLOW.replace(" # v7.0.1", ""))


def test_a_checkout_keeping_its_credentials_fails() -> None:
    _planted("checkout", workflow=GOOD_WORKFLOW.replace("persist-credentials: false", "persist-credentials: true"))


def test_a_checkout_with_no_with_block_fails() -> None:
    _planted("checkout", workflow=GOOD_WORKFLOW.replace("        with:\n          persist-credentials: false\n", ""))


def test_a_checkout_step_written_with_before_uses_is_read_whole() -> None:
    reordered = GOOD_WORKFLOW.replace(
        "      - uses: {c}\n        with:\n          persist-credentials: false\n".format(c=CHECKOUT),
        "      - name: Checkout\n        with:\n          persist-credentials: false\n        uses: {c}\n".format(c=CHECKOUT))
    assert reordered != GOOD_WORKFLOW
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(_tree(pathlib.Path(d), workflow=reordered))
        assert rc == 0, out
    _planted("checkout", workflow=reordered.replace("persist-credentials: false", "persist-credentials: true"))


def test_a_toolchain_literal_in_a_gate_script_fails() -> None:
    _planted("toolchain", script=GOOD_SCRIPT + "cargo " + "+" + "1.99.0 test --workspace\n")


def test_a_toolchain_literal_in_a_workflow_fails() -> None:
    _planted("toolchain", workflow=GOOD_WORKFLOW.replace("run: cargo build --locked", "run: cargo " + "+" + "1.99.0 build --locked"))


def test_an_unpaired_target_dir_in_a_gate_script_fails() -> None:
    _planted("build-dir", script=GOOD_SCRIPT + "\n\nCARGO_TARGET" + "_DIR=/tmp/x cargo check --locked\n")


def test_an_unpaired_target_dir_in_a_workflow_env_fails() -> None:
    _planted("build-dir", workflow=GOOD_WORKFLOW.replace("      CARGO_BUILD_BUILD_DIR: /tmp/t\n", ""))


def test_a_lock_without_zip_8_beside_a_live_zip_ignore_fails() -> None:
    _planted("zip8", lock=LOCK_WITH_ZIP8.split("[[package]]\nname = \"zip\"\nversion = \"9.0.3\"")[0].replace(
        'version = "8.6.0"', 'version = "9.0.3"'))


def test_the_zip_ignores_lifted_with_zip_8_gone_pass() -> None:
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(_tree(pathlib.Path(d), lock=LOCK_WITH_ZIP8.replace('"8.6.0"', '"9.0.2"'),
                             deny='[advisories]\nyanked = "deny"\n# RUSTSEC-2026-0367 lifted\nignore = []\n'))
        assert rc == 0, out


def test_a_repository_without_workflows_is_no_verdict() -> None:
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(pathlib.Path(d))
        assert rc == 2, out


def test_this_repository_passes() -> None:
    root = SCRIPTS.parent
    found = guard.check(root)
    assert found == [], "\n".join(found)


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
