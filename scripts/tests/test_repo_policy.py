#!/usr/bin/env python3.11
"""Tests for `scripts/repo_policy.py`, the repository-policy guard (B-23).

Run with either:
  python3.11 -I scripts/tests/test_repo_policy.py
  python3.11 -m pytest scripts/tests/test_repo_policy.py

Each planted-violation test starts from a small tree the guard passes — its workflow's
setup-python step reading a `.python-version` of `3.11` — plants one violation, and
asserts the guard fails on it and names its rule; the last test runs the guard on this
repository, which must pass. The planted toolchain literal and the planted unpaired
target directory are spelled in two pieces, so that this file — a gate script the guard
reads — is no violation itself.
"""

import importlib.util
import pathlib
import subprocess
import sys
import tempfile

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
GUARD = SCRIPTS / "repo_policy.py"

CHECKOUT = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1"
SETUP_PYTHON = "actions/setup-python@6d4fd22b20465ccc0f7354476588e8df7c9ca0e0 # v7.1.0"
READS_PIN = "        with:\n          python-version-file: .python-version\n"
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
  tooling:
    runs-on: ubuntu-24.04
    steps:
      - uses: {setup_python}
{reads_pin}      - name: Guard
        run: python3 -I scripts/repo_policy.py
""".format(checkout=CHECKOUT, setup_python=SETUP_PYTHON, reads_pin=READS_PIN)

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


def _tree(root: pathlib.Path, workflow: str | None = GOOD_WORKFLOW, script: str = GOOD_SCRIPT,
          lock: str = LOCK_WITH_ZIP8, deny: str = DENY, pin: str | None = "3.11\n") -> pathlib.Path:
    if workflow is not None:  # None: no .github at all
        (root / ".github" / "workflows").mkdir(parents=True)
        (root / ".github" / "workflows" / "ci.yml").write_text(workflow, encoding="utf-8")
    (root / "scripts").mkdir()
    (root / "scripts" / "gate.sh").write_text(script, encoding="utf-8")
    (root / "Cargo.lock").write_text(lock, encoding="utf-8")
    (root / "deny.toml").write_text(deny, encoding="utf-8")
    if pin is not None:  # None: no .python-version at all
        (root / ".python-version").write_bytes(pin.encode("utf-8"))
    return root


def _run(root: pathlib.Path) -> tuple[int, str]:
    proc = subprocess.run([sys.executable, "-I", str(GUARD), str(root)], capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def _planted(rule: str, **kw) -> str:
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(_tree(pathlib.Path(d), **kw))
        assert rc == 1, "rc=%d, expected the guard to fail on a planted %s violation:\n%s" % (rc, rule, out)
        assert (": %s: " % rule) in out, "the failure does not name %s:\n%s" % (rule, out)
        assert out.count(": %s: " % rule) == 1, "one violation planted, more reported:\n%s" % out
        return out


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


def test_a_comment_line_does_not_end_a_checkout_step() -> None:
    # a `#` line at the step's `-` or at column 0 is no YAML: the step reads on past it
    for note in ("      # the token\n", "# the token\n"):
        commented = GOOD_WORKFLOW.replace("        with:\n          persist-credentials: false\n",
                                          "        with:\n" + note + "          persist-credentials: false\n")
        assert commented != GOOD_WORKFLOW
        with tempfile.TemporaryDirectory() as d:
            rc, out = _run(_tree(pathlib.Path(d), workflow=commented))
            assert rc == 0, (note, out)


def test_a_toolchain_literal_in_a_gate_script_fails() -> None:
    _planted("toolchain", script=GOOD_SCRIPT + "cargo " + "+" + "1.99.0 test --workspace\n")


def test_a_toolchain_literal_in_a_workflow_fails() -> None:
    _planted("toolchain", workflow=GOOD_WORKFLOW.replace("run: cargo build --locked", "run: cargo " + "+" + "1.99.0 build --locked"))


def test_an_unpaired_target_dir_in_a_gate_script_fails() -> None:
    _planted("build-dir", script=GOOD_SCRIPT + "\n\nCARGO_TARGET" + "_DIR=/tmp/x cargo check --locked\n")


def test_an_unpaired_target_dir_in_a_workflow_env_fails() -> None:
    _planted("build-dir", workflow=GOOD_WORKFLOW.replace("      CARGO_BUILD_BUILD_DIR: /tmp/t\n", ""))


def test_a_python_version_in_place_of_the_pin_file_fails() -> None:
    # block, quoted-key and flow forms, a literal or a matrix value: the input is the one violation
    for literal in ('python-version: "3.11"', "python-version: 3.11.18", "'python-version': '3.11'",
                    "python-version: ${{ matrix.python }}"):
        out = _planted("python-pin", workflow=GOOD_WORKFLOW.replace("python-version-file: .python-version", literal))
        assert "names a Python" in out, out
    out = _planted("python-pin", workflow=GOOD_WORKFLOW.replace(READS_PIN, '        with: {python-version: "3.11"}\n'))
    assert "names a Python" in out, out


def test_a_setup_python_step_with_both_inputs_fails() -> None:
    # setup-python takes the literal over the file, with only a warning: the literal is refused, by its line
    for both in (READS_PIN + '          python-version: "3.11"\n',
                 '        with:\n          python-version: "3.11"\n          python-version-file: .python-version\n'):
        workflow = GOOD_WORKFLOW.replace(READS_PIN, both)
        out = _planted("python-pin", workflow=workflow)
        n = workflow.splitlines().index('          python-version: "3.11"') + 1
        assert ("ci.yml:%d: python-pin: " % n) in out, out


def test_a_setup_python_step_not_reading_the_pin_file_fails() -> None:
    # no input at all (setup-python's own fallback, with a warning), another file, another path
    for reads in ("", "        with:\n          python-version-file: pyproject.toml\n",
                  "        with:\n          python-version-file: tools/.python-version\n"):
        out = _planted("python-pin", workflow=GOOD_WORKFLOW.replace(READS_PIN, reads))
        assert "does not read the pin" in out, out


def test_a_missing_python_version_file_fails() -> None:
    out = _planted("python-pin", pin=None)
    assert ".python-version:1: python-pin: " in out, out


def test_an_empty_python_version_file_fails() -> None:
    for pin in ("", "\n"):
        out = _planted("python-pin", pin=pin)
        assert ".python-version:1: python-pin: .python-version is empty" in out, (pin, out)


def test_a_python_version_file_of_two_lines_fails() -> None:
    for pin in ("3.11\n3.12\n", "3.11\n\n", "# the scripts' Python\n3.11\n", "3.11\n3.11"):
        out = _planted("python-pin", pin=pin)
        assert ".python-version holds 2 lines" in out, (pin, out)


def test_a_malformed_python_version_file_fails() -> None:
    for pin in ("3.11.x\n", "python3.11\n", "3\n", "3.11.18.1\n", " 3.11\n", "3.11 # the line\n",
                "pypy3.10\n", "3.11\r\n", "3.11.\n"):
        out = _planted("python-pin", pin=pin)
        assert ".python-version:1: python-pin: .python-version reads " in out, (pin, out)


def test_a_python_version_file_that_is_not_utf8_is_no_verdict() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _tree(pathlib.Path(d), pin=None)
        (root / ".python-version").write_bytes(b"3.11\xff\n")
        rc, out = _run(root)
        assert rc == 2, out
        want = "repo_policy: cannot read %s: " % (root.resolve() / ".python-version")
        assert out.startswith(want) and len(out.splitlines()) == 1, out


def test_a_minor_or_a_patch_pin_read_by_setup_python_passes() -> None:
    quoted = GOOD_WORKFLOW.replace("python-version-file: .python-version", 'python-version-file: ".python-version"')
    flow = GOOD_WORKFLOW.replace(READS_PIN, "        with: {python-version-file: .python-version}\n")
    commented = GOOD_WORKFLOW.replace(READS_PIN, READS_PIN + '          # python-version: "3.9.25" before the pin\n')
    assert GOOD_WORKFLOW not in (quoted, flow, commented)
    for workflow, pin in ((GOOD_WORKFLOW, "3.11.18\n"), (GOOD_WORKFLOW, "3.11"), (quoted, "3.11\n"),
                          (flow, "3.11\n"), (commented, "3.11\n")):
        with tempfile.TemporaryDirectory() as d:
            rc, out = _run(_tree(pathlib.Path(d), workflow=workflow, pin=pin))
            assert rc == 0, (pin, out)


def test_a_comment_line_does_not_end_a_setup_python_step() -> None:
    # a `#` line at the step's `-` or at column 0 is no YAML: a literal below it is still the step's input
    for note in ("      # the pin\n", "# the pin\n"):
        workflow = GOOD_WORKFLOW.replace(READS_PIN, READS_PIN + note + '          python-version: "3.12"\n')
        out = _planted("python-pin", workflow=workflow)
        n = workflow.splitlines().index('          python-version: "3.12"') + 1
        assert ("ci.yml:%d: python-pin: " % n) in out, (note, out)


def test_input_names_and_the_action_are_read_in_any_case() -> None:
    # the runner reads an input's name in any case (`Python-Version` is `python-version`)
    out = _planted("python-pin", workflow=GOOD_WORKFLOW.replace(READS_PIN, READS_PIN + "          Python-Version: '3.12'\n"))
    assert "names a Python" in out, out
    upper = GOOD_WORKFLOW.replace("python-version-file: .python-version", "Python-Version-File: .python-version")
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(_tree(pathlib.Path(d), workflow=upper))
        assert rc == 0, out
    # the action's ref is compared in lower case
    ref = SETUP_PYTHON.replace("actions/setup-python@", "Actions/Setup-Python@")
    out = _planted("python-pin", workflow=GOOD_WORKFLOW.replace(SETUP_PYTHON, ref).replace(
        READS_PIN, "        with:\n          python-version: '3.12'\n"))
    assert "names a Python" in out, out


def test_a_lock_without_zip_8_beside_a_live_zip_ignore_fails() -> None:
    _planted("zip8", lock=LOCK_WITH_ZIP8.split("[[package]]\nname = \"zip\"\nversion = \"9.0.3\"")[0].replace(
        'version = "8.6.0"', 'version = "9.0.3"'))


def test_the_zip_ignores_lifted_with_zip_8_gone_pass() -> None:
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(_tree(pathlib.Path(d), lock=LOCK_WITH_ZIP8.replace('"8.6.0"', '"9.0.2"'),
                             deny='[advisories]\nyanked = "deny"\n# RUSTSEC-2026-0367 lifted\nignore = []\n'))
        assert rc == 0, out


def test_a_lock_that_is_not_toml_beside_a_live_zip_ignore_is_no_verdict() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _tree(pathlib.Path(d), lock='[[package]\nname = "zip"\nversion = "8.6.0"\n')
        rc, out = _run(root)
        assert rc == 2, out
        want = "repo_policy: cannot read %s: not TOML: " % (root.resolve() / "Cargo.lock")
        assert out.startswith(want) and len(out.splitlines()) == 1, out


def test_a_lock_whose_packages_are_not_tables_beside_a_live_zip_ignore_is_no_verdict() -> None:
    for lock in ("package = 1\n", "package = [1]\n"):
        with tempfile.TemporaryDirectory() as d:
            root = _tree(pathlib.Path(d), lock=lock)
            rc, out = _run(root)
            assert rc == 2, (lock, out)
            want = ("repo_policy: cannot read %s: not a Cargo.lock: [[package]] is not an array of tables"
                    % (root.resolve() / "Cargo.lock"))
            assert out.strip() == want and len(out.splitlines()) == 1, (lock, out)


def test_the_zip_majors_are_read_from_the_lock_and_none_without_one() -> None:
    with tempfile.TemporaryDirectory() as d:
        lock = pathlib.Path(d) / "Cargo.lock"
        assert guard.locked_zip_majors(lock) == [], "a missing Cargo.lock reads as no zip"
        lock.write_text(LOCK_WITH_ZIP8, encoding="utf-8")
        majors = guard.locked_zip_majors(lock)
        assert majors == ["8", "9"], majors


def test_a_repository_without_workflows_checks_its_gate_scripts() -> None:
    # no remote CI (the owner, 2026-11-06): zero workflows is a tree to check, not a usage error
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(_tree(pathlib.Path(d), workflow=None))
        assert rc == 0, out
        assert "0 workflow(s), 1 gate script(s), 0 violation(s)" in out, out
    _planted("toolchain", workflow=None, script=GOOD_SCRIPT + "cargo " + "+" + "1.99.0 test --workspace\n")


def test_a_root_that_is_not_a_directory_is_no_verdict() -> None:
    with tempfile.TemporaryDirectory() as d:
        rc, out = _run(pathlib.Path(d) / "missing")
        assert rc == 2 and "is not a directory" in out, out


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
