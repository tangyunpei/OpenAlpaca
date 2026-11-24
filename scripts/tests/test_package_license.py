#!/usr/bin/env python3.11
"""Tests for the release packaging's LICENSE: the refusal of a tree whose root LICENSE
is missing or empty in `scripts/release/package-macos.sh` and `package-linux.sh`, and
the app bundle's `licenseFile` and `copyright` keys in
`apps/openalpaca-gui/src-tauri/tauri.conf.json` (the owner, 2026-11-17: "add the
bundle's licenseFile and copyright keys, and refuse a build with no LICENSE").

Run with either:
  python3.11 -I scripts/tests/test_package_license.py
  python3.11 -m pytest scripts/tests/test_package_license.py

A refusal case copies a packaging script into a temporary tree (`<tree>/scripts/release/`,
so the script's REPO_ROOT is the tree) and runs it whole. The tree holds no package.json,
no lockfile and no notices, so a run that passes the LICENSE check stops at the next
check, before anything is built. `package-windows.ps1`'s twin, `Assert-License`, is
tested by Pester (`scripts/release/tests`), run by hand on Windows.
"""

import json
import pathlib
import shutil
import subprocess
import sys
import tempfile

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
REPO = SCRIPTS.parent
RELEASE = SCRIPTS / "release"
SHELL_SCRIPTS = ("package-macos.sh", "package-linux.sh")
SRC_TAURI = REPO / "apps" / "openalpaca-gui" / "src-tauri"


def _tree(root: pathlib.Path, script: str, license_text: str | None) -> pathlib.Path:
    """A tree with the one script under scripts/release/ and, unless None, a LICENSE."""
    (root / "scripts" / "release").mkdir(parents=True)
    copy = root / "scripts" / "release" / script
    shutil.copy2(RELEASE / script, copy)
    if license_text is not None:
        (root / "LICENSE").write_text(license_text, encoding="utf-8")
    return copy


def _run(copy: pathlib.Path) -> tuple[int, str]:
    proc = subprocess.run(["bash", str(copy)], capture_output=True, text=True, timeout=60)
    return proc.returncode, proc.stdout + proc.stderr


def _sourced(copy: pathlib.Path) -> tuple[int, str]:
    script = 'source "$1"; require_license && echo ACCEPTED'
    proc = subprocess.run(["bash", "-c", script, "bash", str(copy)], capture_output=True, text=True,
                          timeout=60)
    return proc.returncode, proc.stdout + proc.stderr


def test_packaging_stops_before_building_when_the_license_is_missing_naming_it() -> None:
    for script in SHELL_SCRIPTS:
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d).resolve()
            rc, out = _run(_tree(root, script, None))
            assert rc == 1, (script, rc, out)
            assert f"No LICENSE at {root}/LICENSE" in out, (script, out)
            assert "==> " not in out, (script, "nothing is checked or built first", out)


def test_packaging_stops_before_building_when_the_license_is_empty_naming_it() -> None:
    for script in SHELL_SCRIPTS:
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d).resolve()
            rc, out = _run(_tree(root, script, ""))
            assert rc == 1, (script, rc, out)
            assert f"{root}/LICENSE is empty" in out, (script, out)
            assert "==> " not in out, (script, "nothing is checked or built first", out)


def test_a_license_with_text_passes_the_check() -> None:
    for script in SHELL_SCRIPTS:
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d).resolve()
            copy = _tree(root, script, "MIT License\n")
            rc, out = _sourced(copy)
            assert rc == 0 and "ACCEPTED" in out, (script, rc, out)
            # Run whole, the script passes the LICENSE check and stops at a later one: the
            # tree has no package.json, or is not the script's platform.
            rc, out = _run(copy)
            assert rc == 1 and "LICENSE" not in out, (script, rc, out)


def test_a_sourced_script_builds_nothing() -> None:
    """`source` defines the functions and returns before any check, as package-macos.sh's
    own comment says (the seam the refusal cases above and test_gen_notices.py use)."""
    for script in SHELL_SCRIPTS:
        with tempfile.TemporaryDirectory() as d:
            copy = _tree(pathlib.Path(d).resolve(), script, None)
            proc = subprocess.run(["bash", "-c", 'source "$1"; echo SOURCED', "bash", str(copy)],
                                  capture_output=True, text=True, timeout=60)
            assert proc.returncode == 0 and proc.stdout == "SOURCED\n", (script, proc)


def test_the_app_bundle_names_the_repositorys_license_and_its_holder_line() -> None:
    bundle = json.loads((SRC_TAURI / "tauri.conf.json").read_text(encoding="utf-8"))["bundle"]
    license_lines = (REPO / "LICENSE").read_text(encoding="utf-8").splitlines()
    assert bundle["copyright"] == license_lines[2], (bundle.get("copyright"), license_lines[2])
    assert bundle["copyright"].startswith("Copyright (c) "), bundle["copyright"]
    # Tauri resolves a bundle path from the directory that holds tauri.conf.json, as the
    # resources map's own LICENSE entry does.
    assert (SRC_TAURI / bundle["licenseFile"]).resolve() == (REPO / "LICENSE").resolve(), bundle
    assert bundle["licenseFile"] in bundle["resources"], (bundle["licenseFile"], bundle["resources"])


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
