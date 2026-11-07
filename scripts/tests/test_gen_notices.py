#!/usr/bin/env python3.11
"""Tests for `scripts/release/gen_notices.py`, the THIRD-PARTY-NOTICES generator (B-22),
and for `scripts/release/package-macos.sh`'s refusal of a stale notices file.

Run with either:
  python3.11 -I scripts/tests/test_gen_notices.py
  python3.11 -m pytest scripts/tests/test_gen_notices.py

Every case runs on fixtures in a temporary directory: a recorded cargo-about JSON, a small
bun.lock, a fake node_modules and SPDX texts; a generation case runs the generator with a
stand-in cargo-about, a bash script that prints that JSON. None runs cargo-about or bun,
and none reads the repository's own lockfiles or node_modules, so the file runs offline
in CI's `tooling` job. One case reads the repository's committed THIRD-PARTY-NOTICES and
deny.toml, as CI's licence step does, and one its vendored texts of Apache-2.0 and the
LLVM exception. The refusal cases source package-macos.sh in bash, as its own comment
says to, point REPO_ROOT at a fixture tree and call the check; bash and shasum are all
they need.
"""

import hashlib
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]
GEN = SCRIPTS / "release" / "gen_notices.py"
PACKAGE_MACOS = SCRIPTS / "release" / "package-macos.sh"

MIT_TEXT = "MIT License\n\nCopyright (c) <year> <copyright holders>\n\nPermission is hereby granted.\n"
APACHE_TEXT = "Apache License\nVersion 2.0, January 2004\n"

# A bun.lock in Bun's text form (JSON with trailing commas): the app's workspace depends on
# `a`; `a` depends on `b` in a version nested under it (`a/b`) while the hoisted `b` is
# another's; `a/b` optionally depends on `c`; `a` has an optional peer `e` the lock holds
# and an optional peer `f` it does not. `d`, `x` and `vite` are reached only from
# devDependencies; `vite` is the one bundled tool (gen_notices.BUNDLED_TOOLS).
BUN_LOCK = """\
{
  "lockfileVersion": 1,
  "workspaces": {
    "": {
      "name": "root",
    },
    "apps/openalpaca-gui": {
      "name": "openalpaca-gui",
      "dependencies": {
        "a": "1.0.0",
      },
      "devDependencies": {
        "d": "1.0.0",
        "vite": "8.3.5",
      },
    },
  },
  "packages": {
    "a": ["a@1.0.0", "", { "dependencies": { "b": "^2.0.0" }, "peerDependencies": { "e": "*", "f": "*" }, "optionalPeers": ["e", "f"] }, "sha512-a"],
    "a/b": ["b@2.0.0", "", { "optionalDependencies": { "c": "^1.0.0" } }, "sha512-ab"],
    "b": ["b@1.0.0", "", {}, "sha512-b"],
    "c": ["c@1.0.0", "", {}, "sha512-c"],
    "d": ["d@1.0.0", "", { "dependencies": { "x": "^1.0.0" } }, "sha512-d"],
    "e": ["e@1.0.0", "", {}, "sha512-e"],
    "x": ["x@1.0.0", "", {}, "sha512-x"],
    "vite": ["vite@8.3.5", "", {}, "sha512-v"],
  },
}
"""

# vite's LICENSE.md in its own form: its MIT notice, then the sections on the dependencies
# vite bundles into its own build, which are not code it writes into the app's bundle.
VITE_NOTICE = """\
# Vite core license
Vite is released under the MIT license:

MIT License

Copyright (c) 2019-present, VoidZero Inc. and Vite contributors

Permission is hereby granted.

"""
VITE_LICENCE = VITE_NOTICE + """\
# Licenses of bundled dependencies
The published Vite artifact additionally contains code with the following licenses:
BSD-2-Clause, CC0-1.0, ISC, MIT

# Bundled dependencies:
## picomatch
Picomatch's own licence
"""


def _load():
    if not GEN.is_file():
        raise AssertionError("%s does not exist" % GEN)
    spec = importlib.util.spec_from_file_location("gen_notices", GEN)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_GEN = None


def gen():
    """The generator module, loaded once; a missing module fails each case on its own."""
    global _GEN
    if _GEN is None:
        _GEN = _load()
    return _GEN


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _write(path: pathlib.Path, text: str, mode: str = "w") -> pathlib.Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, mode, encoding="utf-8", newline="") as fh:
        fh.write(text)
    return path


def _spdx(root: pathlib.Path) -> pathlib.Path:
    d = root / "scripts" / "release" / "spdx"
    _write(d / "MIT.txt", MIT_TEXT)
    _write(d / "Apache-2.0.txt", APACHE_TEXT)
    return root


def _npm_package(root: pathlib.Path, name: str, version: str, licence, files: dict, suffix: str = "") -> None:
    """A package in Bun's isolated layout; `licence` None writes a package.json naming none."""
    d = root / "node_modules" / ".bun" / ("%s@%s%s" % (name.replace("/", "+"), version, suffix)) / "node_modules" / name
    meta = {"name": name, "version": version}
    if licence is not None:
        meta["license"] = licence
    _write(d / "package.json", json.dumps(meta))
    for fname, text in files.items():
        _write(d / fname, text)


def _vite(root: pathlib.Path, licence="MIT", files=None) -> None:
    """vite 8.3.5 installed as Bun installs it (a peer-hash suffix on its directory), with
    vite's LICENSE.md unless `files` says otherwise."""
    _npm_package(root, "vite", "8.3.5", licence, {"LICENSE.md": VITE_LICENCE} if files is None else files,
                 suffix="+9f9b3119fbf6d926")


def _bundled_line(text: str, name: str, version: str) -> list:
    """The lines of the licence texts beside a package that call it a build tool whose
    runtime code the app's bundle carries."""
    prefix = "  %s %s (" % (name, version)
    return [line for line in text.split("\n")
            if line.startswith(prefix) and "build tool" in line and "bundle carries" in line]


def _crate(root: pathlib.Path, name: str, version: str, files: dict) -> str:
    d = root / "registry" / ("%s-%s" % (name, version))
    _write(d / "Cargo.toml", "[package]\nname = %r\n" % name)
    for fname, text in files.items():
        _write(d / fname, text)
    return str(d / "Cargo.toml")


def _about(root: pathlib.Path) -> dict:
    """cargo-about's `--format json` shape: `id` identified from a crate file (`source_path`
    set), `ok` given a synthesized text though it ships a file cargo-about did not identify,
    and `bare` given a synthesized text because it ships none."""
    crates = {
        ("id", "1.0.0"): _crate(root, "id", "1.0.0", {"LICENSE-MIT": "MIT License\r\n\r\nCopyright (c) 2026 Id  \r\n"}),
        ("ok", "0.2.0"): _crate(root, "ok", "0.2.0", {"license-mit": "MIT License\n\nCopyright (c) Ok Corp.\n"}),
        ("bare", "3.0.0"): _crate(root, "bare", "3.0.0", {}),
    }
    def used(*keys):
        return [{"crate": {"name": n, "version": v}, "path": None} for (n, v) in keys]
    return {
        "overview": [],
        "licenses": [
            {"name": "MIT License", "id": "MIT", "text": "MIT License\n\nCopyright (c) 2026 Id\n",
             "source_path": str(pathlib.Path(crates[("id", "1.0.0")]).parent / "LICENSE-MIT"),
             "used_by": used(("id", "1.0.0"))},
            {"name": "MIT License", "id": "MIT", "text": "synthesized\n", "source_path": None,
             "used_by": used(("ok", "0.2.0"), ("bare", "3.0.0"))},
        ],
        "crates": [
            {"package": {"name": n, "version": v, "license": "MIT OR  Apache-2.0", "manifest_path": m,
                         "source": "registry+https://github.com/rust-lang/crates.io-index"},
             "license": "MIT OR Apache-2.0"}
            for (n, v), m in crates.items()
        ],
    }


def _tree(root: pathlib.Path) -> pathlib.Path:
    """A tree with both lockfiles and a THIRD-PARTY-NOTICES that records their hashes."""
    _write(root / "Cargo.lock", "version = 4\n")
    _write(root / "bun.lock", BUN_LOCK)
    _write(root / "THIRD-PARTY-NOTICES", "THIRD-PARTY-NOTICES\n\nCargo.lock sha256 %s\nbun.lock sha256 %s\n"
           % (_sha(b"version = 4\n"), _sha(BUN_LOCK.encode())))
    return root


def test_the_header_names_the_generator_and_each_lockfile_hash_and_no_time() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _tree(pathlib.Path(d))
        lines = gen().hash_lines(root)
        assert lines == ["Cargo.lock sha256 %s" % _sha(b"version = 4\n"),
                         "bun.lock sha256 %s" % _sha(BUN_LOCK.encode())], lines
        text = gen().render(lines, gen().Half(), gen().Half())
        assert text.startswith("THIRD-PARTY-NOTICES\n"), text[:40]
        assert "Generated by scripts/release/gen_notices.py; do not edit by hand." in text, text
        for line in lines:
            assert ("\n%s\n" % line) in text, line
        assert gen().recorded_hashes(text) == {"Cargo.lock": _sha(b"version = 4\n"), "bun.lock": _sha(BUN_LOCK.encode())}
        assert "cargo-about %s" % gen().CARGO_ABOUT_VERSION in text, "the Rust tool's version is in the header"
        assert "spdx/license-list-data %s" % gen().SPDX_TAG in text, "the substituted texts' source is in the header"
        assert "2026-" not in text and "UTC" not in text, "no timestamp"


def test_the_bun_lock_closure_follows_the_apps_dependencies_and_not_its_dev_dependencies() -> None:
    with tempfile.TemporaryDirectory() as d:
        lock = _write(pathlib.Path(d) / "bun.lock", BUN_LOCK)
        doc = gen().load_bun_lock(lock)
        keys = gen().bun_closure(doc, "apps/openalpaca-gui")
        assert keys == ["a", "a/b", "c", "e"], keys


def test_licence_files_are_the_packages_own_and_an_spdx_document_is_not_one() -> None:
    with tempfile.TemporaryDirectory() as d:
        pkg = pathlib.Path(d)
        for name in ("LICENSE", "LICENCE.md", "license-mit", "COPYING", "NOTICE", "COPYRIGHT",
                     "LICENSE.spdx", "README.md", "package.json"):
            _write(pkg / name, "text of %s\n" % name)
        (pkg / "LICENSES").mkdir()
        found = [name for name, _ in gen().licence_files(pkg)]
        assert found == ["COPYING", "COPYRIGHT", "LICENCE.md", "LICENSE", "NOTICE", "license-mit"], found


def test_a_licence_directorys_files_are_licence_files_and_a_nested_licence_is_not() -> None:
    with tempfile.TemporaryDirectory() as d:
        pkg = pathlib.Path(d)
        for name in ("LICENSE", ".licenses/Alice-MIT", ".licenses/Bob-Apache", "LICENSES/MIT.txt",
                     "LICENSES/MIT.spdx", "src/LICENSE", "docs/licenses.md"):
            _write(pkg / name, "text of %s\n" % name)
        found = [name for name, _ in gen().licence_files(pkg)]
        assert found == [".licenses/Alice-MIT", ".licenses/Bob-Apache", "LICENSE", "LICENSES/MIT.txt"], found


def test_texts_are_normalized_so_the_file_has_no_trailing_space_or_crlf() -> None:
    assert gen().normalize("﻿\r\n  MIT License  \r\n\r\nCopyright\t\r\n\r\n\r\n") == "  MIT License\n\nCopyright\n"
    assert gen().normalize("a\rb\n") == "a\nb\n"


def test_a_js_package_with_no_licence_file_gets_the_marked_standard_text_of_each_id() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        lock = _write(root / "bun.lock", BUN_LOCK)
        doc = gen().load_bun_lock(lock)
        _npm_package(root, "a", "1.0.0", "MIT", {"LICENSE": "MIT License\n\nCopyright (c) A\n"}, suffix="+0123abcd")
        _npm_package(root, "b", "2.0.0", "MIT OR Apache-2.0", {"LICENSE.spdx": "SPDXVersion: SPDX-2.1\n"})
        _npm_package(root, "c", "1.0.0", "MIT", {"LICENSE": "MIT License\r\n\r\nCopyright (c) A  \r\n"})
        _npm_package(root, "e", "1.0.0", "(MIT)", {"LICENSE": "e's licence\n"})
        half = gen().js_half(root, doc, gen().bun_closure(doc, "apps/openalpaca-gui"), ["MIT", "Apache-2.0"])
        assert half.packages == [("a", "1.0.0", "MIT"), ("b", "2.0.0", "MIT OR Apache-2.0"),
                                 ("c", "1.0.0", "MIT"), ("e", "1.0.0", "(MIT)")], half.packages
        substituted = {t: g for t, g in half.groups.items() if g.substituted}
        assert set(substituted) == {MIT_TEXT, APACHE_TEXT}, sorted(substituted)
        for g in substituted.values():
            assert g.substituted == {("b", "2.0.0")}, g.substituted
        out = gen().render(["Cargo.lock sha256 0", "bun.lock sha256 0"], gen().Half(), half)
        assert "MIT, the SPDX standard text (substituted)" in out, out
        assert "  b 2.0.0 (ships no licence file)" in out, out
        assert half.substituted_count() == 1


def test_identical_texts_are_given_once_with_every_package_and_in_sorted_order() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        doc = gen().load_bun_lock(_write(root / "bun.lock", BUN_LOCK))
        _npm_package(root, "a", "1.0.0", "MIT", {"LICENSE": "MIT License\n\nCopyright (c) A\n"})
        _npm_package(root, "b", "2.0.0", "MIT", {"LICENSE": "Zeta licence\n"})
        _npm_package(root, "c", "1.0.0", "MIT", {"LICENSE": "MIT License\r\n\r\nCopyright (c) A  \r\n"})
        _npm_package(root, "e", "1.0.0", "MIT", {"LICENSE": "Zeta licence\n", "NOTICE": "Alpha notice\n"})
        half = gen().js_half(root, doc, gen().bun_closure(doc, "apps/openalpaca-gui"), ["MIT"])
        assert len(half.groups) == 3, sorted(half.groups)
        a_text = "MIT License\n\nCopyright (c) A\n"
        assert half.groups[a_text].used_by == {("a", "1.0.0"), ("c", "1.0.0")}, half.groups[a_text].used_by
        out = gen().render(["Cargo.lock sha256 0", "bun.lock sha256 0"], gen().Half(), half)
        assert out.count("Copyright (c) A") == 1, "an identical text is written once"
        body = out[out.index("Licence texts: JavaScript packages"):]
        order = [body.index(s) for s in ("LICENSE as shipped\n  a 1.0.0\n  c 1.0.0\n",
                                         "LICENSE as shipped\n  b 2.0.0\n  e 1.0.0\n",
                                         "NOTICE as shipped\n  e 1.0.0\n")]
        assert order == sorted(order), order
        listing = out[out.index("JavaScript packages\n=="):]
        assert listing.index("a 1.0.0: MIT") < listing.index("b 2.0.0: MIT") < listing.index("e 1.0.0: MIT")


def test_cargo_about_output_gives_identified_texts_shipped_files_and_substitutions_and_no_path() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        about = _about(root)
        half = gen().rust_half(root, about, ALLOWED)
        assert half.packages == [("bare", "3.0.0", "MIT OR Apache-2.0"), ("id", "1.0.0", "MIT OR Apache-2.0"),
                                 ("ok", "0.2.0", "MIT OR Apache-2.0")], half.packages
        groups = half.groups
        assert groups["MIT License\n\nCopyright (c) 2026 Id\n"].labels == {"MIT"}
        assert groups["MIT License\n\nCopyright (c) Ok Corp.\n"].labels == {"license-mit as shipped"}
        assert groups["MIT License\n\nCopyright (c) Ok Corp.\n"].used_by == {("ok", "0.2.0")}
        assert groups[MIT_TEXT].substituted == {("bare", "3.0.0")}
        assert "synthesized\n" not in groups, "cargo-about's own fallback text is never written"
        assert half.substituted_count() == 1
        out = gen().render(["Cargo.lock sha256 0", "bun.lock sha256 0"], half, gen().Half())
        assert d not in out and "/registry/" not in out, "no local path reaches the file"
        assert "ok 0.2.0: MIT OR Apache-2.0" in out, "the declared expression, its spacing collapsed"


def test_a_text_counts_as_read_only_from_the_crates_own_licence_file() -> None:
    """cargo-about's text from a source file's licence header, or from another crate's
    file the crate does not ship, is not the crate's: the crate falls back to its own
    licence files (a licence directory's among them), or with none to the standard text.
    A text several crates share under one crate's path stays read for each that ships it."""
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        hdr = _crate(root, "hdr", "1.0.0", {"LICENSE-MIT": "MIT License\n\nCopyright (c) Hdr\n",
                                            "src/io.rs": "// Copyright Hdr.\n// ISC header\npub mod der;\n"})
        owner = _crate(root, "owner", "1.0.0", {"LICENSE": "Shared MIT text\n"})
        sharer = _crate(root, "sharer", "2.0.0", {"LICENSE.txt": "Shared MIT text\r\n"})
        borrower = _crate(root, "borrower", "0.38.0", {})
        gp = _crate(root, "gp", "2.0.0", {"LICENSE": "See .licenses/ for the inherited texts.\n",
                                          ".licenses/Alice-MIT": "MIT License\n\nCopyright (c) Alice\n"})
        def used(*keys):
            return [{"crate": {"name": n, "version": v}, "path": None} for (n, v) in keys]
        about = {
            "licenses": [
                {"id": "MIT", "text": "MIT License\n\nCopyright (c) Hdr\n",
                 "source_path": str(pathlib.Path(hdr).parent / "LICENSE-MIT"), "used_by": used(("hdr", "1.0.0"))},
                {"id": "ISC", "text": "// Copyright Hdr.\n// ISC header\npub mod der;\n",
                 "source_path": str(pathlib.Path(hdr).parent / "src" / "io.rs"), "used_by": used(("hdr", "1.0.0"))},
                {"id": "MIT", "text": "Shared MIT text\n", "source_path": str(pathlib.Path(owner).parent / "LICENSE"),
                 "used_by": used(("owner", "1.0.0"), ("sharer", "2.0.0"), ("borrower", "0.38.0"))},
                {"id": "MIT", "text": "synthesized\n", "source_path": None, "used_by": used(("gp", "2.0.0"))},
            ],
            "crates": [
                {"package": {"name": n, "version": v, "license": lic, "manifest_path": m}, "license": lic}
                for n, v, lic, m in (("hdr", "1.0.0", "MIT AND ISC", hdr), ("owner", "1.0.0", "MIT", owner),
                                     ("sharer", "2.0.0", "MIT", sharer), ("borrower", "0.38.0", "MIT", borrower),
                                     ("gp", "2.0.0", "MIT", gp))
            ],
        }
        groups = gen().rust_half(root, about, ALLOWED + ["ISC"]).groups
        assert not any("pub mod der" in t for t in groups), "a source file is never given as a licence text"
        assert groups["MIT License\n\nCopyright (c) Hdr\n"].labels == {"MIT", "LICENSE-MIT as shipped"}, \
            groups["MIT License\n\nCopyright (c) Hdr\n"].labels
        shared = groups["Shared MIT text\n"]
        assert shared.used_by == {("owner", "1.0.0"), ("sharer", "2.0.0")}, shared.used_by
        assert groups[MIT_TEXT].substituted == {("borrower", "0.38.0")}, groups[MIT_TEXT].substituted
        assert groups["MIT License\n\nCopyright (c) Alice\n"].labels == {".licenses/Alice-MIT as shipped"}
        assert groups["See .licenses/ for the inherited texts.\n"].used_by == {("gp", "2.0.0")}


def test_a_licence_id_with_no_vendored_text_has_none_and_is_no_refusal() -> None:
    """The owner's ruling of 2026-11-01: a missing licence text never stops the notices.
    A vendored text is found by its exact name, so a case-insensitive file system reads
    the same tree as any other."""
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        assert gen().spdx_text(root, "ISC") is None, "no spdx directory at all"
        _spdx(root)
        assert gen().spdx_text(root, "ISC") is None
        assert gen().spdx_text(root, "MIT") == MIT_TEXT
        assert gen().spdx_text(root, "mit") is None, "a vendored text is found by its exact name"


def test_a_crate_with_no_licence_file_and_no_vendored_text_is_listed_marked_and_counted() -> None:
    """The Rust half's path: a crate that ships no licence file, under an id with no
    vendored text, is listed with its expression and marked, never refused; one with a
    text substituted for one id and none for another counts in both header lines."""
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        lone = _crate(root, "lone", "1.0.0", {})
        both = _crate(root, "both", "2.0.0", {})
        def used(*keys):
            return [{"crate": {"name": n, "version": v}, "path": None} for (n, v) in keys]
        about = {
            "licenses": [
                {"id": "ISC", "text": "synthesized ISC\n", "source_path": None,
                 "used_by": used(("lone", "1.0.0"), ("both", "2.0.0"))},
                {"id": "MIT", "text": "synthesized\n", "source_path": None, "used_by": used(("both", "2.0.0"))},
            ],
            "crates": [
                {"package": {"name": n, "version": v, "license": lic, "manifest_path": m}, "license": lic}
                for n, v, lic, m in (("lone", "1.0.0", "ISC", lone), ("both", "2.0.0", "MIT AND ISC", both))
            ],
        }
        half = gen().rust_half(root, about, ALLOWED + ["ISC"])
        assert half.packages == [("both", "2.0.0", "MIT AND ISC"), ("lone", "1.0.0", "ISC")], half.packages
        assert half.groups[MIT_TEXT].substituted == {("both", "2.0.0")}
        assert "synthesized ISC\n" not in half.groups, "cargo-about's own fallback text is never written"
        out = gen().render(["Cargo.lock sha256 0", "bun.lock sha256 0"], half, gen().Half())
        assert "\nRust crates given a substituted text: 1\n" in out, out
        assert "\nRust crates missing a licence text: 2\n" in out, out
        assert "\nJavaScript packages missing a licence text: 0\n" in out, out
        assert "\nlone 1.0.0: ISC\n" in out and "\nboth 2.0.0: MIT AND ISC\n" in out, out
        rule = "-" * 72
        assert ("\n%s\nISC: no licence text available\n"
                "  both 2.0.0 (ships no licence file; no standard text available)\n"
                "  lone 1.0.0 (ships no licence file; no standard text available)\n%s\n" % (rule, rule)) in out, out
        assert "Rust licence texts: 1\n" in out, "a missing text is no licence text"


def _ref_crate(root: pathlib.Path, name: str, version: str, licence: str, files: dict) -> dict:
    """A crate as cargo-about 0.9.2 writes one it names no licence for: in `crates`, in
    no licence's `used_by` (what it wrote for a scratch crate under an accepted
    LicenseRef-Foo that ships no licence file)."""
    return {"package": {"name": name, "version": version, "license": licence,
                        "manifest_path": _crate(root, name, version, files)}, "license": licence}


def test_a_crate_cargo_about_names_no_licence_for_ships_under_its_expression_never_refused() -> None:
    """The owner's ruling of 2026-11-01 on the Rust half: a crate under an accepted
    LicenseRef- id is given no licence by cargo-about, which has no text for it. It ships
    under the ids its expression's allowed branches name: with no licence file it is
    listed, marked and counted; with one, that file is given as shipped. Only a crate whose
    expression gives no allowed id is still refused."""
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        accepted = ALLOWED + ["LicenseRef-Foo"]
        about = {"licenses": [], "crates": [_ref_crate(root, "refonly", "1.0.0", "LicenseRef-Foo", {}),
                                            _ref_crate(root, "refown", "2.0.0", "LicenseRef-Foo",
                                                       {"LICENSE": "The Foo licence\n"})]}
        half = gen().rust_half(root, about, accepted)
        assert half.packages == [("refonly", "1.0.0", "LicenseRef-Foo"), ("refown", "2.0.0", "LicenseRef-Foo")]
        assert half.missing == {"LicenseRef-Foo": {("refonly", "1.0.0")}}, half.missing
        assert half.groups["The Foo licence\n"].used_by == {("refown", "2.0.0")}, half.groups
        out = gen().render(["Cargo.lock sha256 0", "bun.lock sha256 0"], half, gen().Half())
        assert "\nRust crates missing a licence text: 1\n" in out, out
        rule = "-" * 72
        assert ("\n%s\nLicenseRef-Foo: no licence text available\n"
                "  refonly 1.0.0 (ships no licence file; no standard text available)\n%s\n" % (rule, rule)) in out, out
        for licence in ("", "GPL-3.0-only", "MIT or Apache-2.0"):
            about = {"licenses": [], "crates": [_ref_crate(root, "refonly", "1.0.0", licence, {})]}
            try:
                gen().rust_half(root, about, accepted)
            except gen().NoVerdict as e:
                assert "cargo-about gave refonly 1.0.0 no licence" in str(e), (licence, str(e))
            else:
                raise AssertionError("a crate with no allowed licence, %r, was not refused" % licence)


def test_the_accepted_licences_must_be_deny_tomls_allow_list() -> None:
    deny = '[licenses]\nallow = ["Apache-2.0", "MIT", "ISC"]\n'
    gen().check_accepted('accepted = ["MIT", "ISC", "Apache-2.0"]\n', deny)
    try:
        gen().check_accepted('accepted = ["MIT", "Apache-2.0"]\n', deny)
    except gen().NoVerdict as e:
        assert "ISC" in str(e), str(e)
    else:
        raise AssertionError("an accepted list narrower than deny.toml's was accepted")


def test_the_same_input_renders_the_same_bytes() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        first = gen().render(["Cargo.lock sha256 0", "bun.lock sha256 0"], gen().rust_half(root, _about(root), ALLOWED), gen().Half())
        about = _about(root)
        about["licenses"].reverse()
        about["crates"].reverse()
        second = gen().render(["Cargo.lock sha256 0", "bun.lock sha256 0"], gen().rust_half(root, about, ALLOWED), gen().Half())
        assert first == second


def _check(root: pathlib.Path, *more: str) -> tuple[int, str]:
    cmd = [sys.executable, "-I", str(GEN), "--check", "--root", str(root)] + list(more)
    proc = subprocess.run(cmd, capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def test_only_the_headers_exact_hash_lines_are_records() -> None:
    """A hash line counts only in the header (before the first rule), at column 0 with
    single spaces, as the generator writes it; the packaging check reads the same lines."""
    with tempfile.TemporaryDirectory() as d:
        root = _tree(pathlib.Path(d))
        cargo, bun = _sha(b"version = 4\n"), _sha(BUN_LOCK.encode())
        rule = "=" * 72
        current = "X\n\nCargo.lock sha256 %s\nbun.lock sha256 %s\n%s\nbody\n" % (cargo, bun, rule)
        assert gen().stale_locks(root, current) == []
        planted = "X\n\nCargo.lock sha256 %s\nbun.lock sha256 %s\n%s\nCargo.lock sha256 %s\n" % (cargo, bun, rule, cargo)
        assert gen().stale_locks(root, planted) == [], "a line after the rule is no record"
        cases = {
            "after the rule only": "X\nbun.lock sha256 %s\n%s\nCargo.lock sha256 %s\n" % (bun, rule, cargo),
            "indented": "X\n  Cargo.lock sha256 %s\nbun.lock sha256 %s\n" % (cargo, bun),
            "tab-separated": "X\nCargo.lock\tsha256 %s\nbun.lock sha256 %s\n" % (cargo, bun),
        }
        for label, text in cases.items():
            stale = gen().stale_locks(root, text)
            assert len(stale) == 1 and "Cargo.lock sha256 <hex>" in stale[0], (label, stale)
            _write(root / "THIRD-PARTY-NOTICES", text)
            rc, out = _refusal(root)
            assert rc == 1 and 'single "Cargo.lock sha256' in out and "ACCEPTED" not in out, (label, rc, out)
        for label, text in (("current", current), ("planted after the rule", planted)):
            _write(root / "THIRD-PARTY-NOTICES", text)
            rc, out = _refusal(root)
            assert rc == 0 and "ACCEPTED" in out, (label, rc, out)


def test_check_names_the_stale_lockfile() -> None:
    for lock in ("Cargo.lock", "bun.lock"):
        with tempfile.TemporaryDirectory() as d:
            root = _tree(pathlib.Path(d))
            _write(root / lock, "\n", mode="a")
            rc, out = _check(root)
            assert rc == 1, (rc, out)
            assert "%s sha256" % lock in out and "stale" in out, out
            other = "bun.lock" if lock == "Cargo.lock" else "Cargo.lock"
            assert "%s sha256" % other not in out, out


def _check_hashes(root: pathlib.Path, *more: str) -> tuple[int, str]:
    cmd = [sys.executable, "-I", str(GEN), "--check-hashes", "--root", str(root)] + list(more)
    proc = subprocess.run(cmd, capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def test_check_hashes_reads_the_two_hashes_alone_regenerates_nothing_and_is_a_mode_of_its_own() -> None:
    """CI's step: the header's hash lines against the tree's lockfiles. The fixture holds no
    deny.toml, about.toml, node_modules or cargo-about, so --check, which regenerates, has
    no verdict on it (exit 2): exit 0 here shows nothing else was read."""
    with tempfile.TemporaryDirectory() as d:
        root = _tree(pathlib.Path(d))
        before = (root / "THIRD-PARTY-NOTICES").read_bytes()
        rc, out = _check(root)
        assert rc == 2, ("--check regenerates", rc, out)
        rc, out = _check_hashes(root)
        assert rc == 0 and "Cargo.lock and bun.lock" in out, (rc, out)
        assert (root / "THIRD-PARTY-NOTICES").read_bytes() == before, "the check writes nothing"
        assert sorted(p.name for p in root.iterdir()) == ["Cargo.lock", "THIRD-PARTY-NOTICES", "bun.lock"]
        for other in ("--check", "--check-licences"):
            rc, out = _check_hashes(root, other)
            assert rc == 2 and "Usage" in out, (other, rc, out)


def test_check_hashes_names_each_stale_lockfile_and_the_command_that_regenerates_the_file() -> None:
    for changed in (("Cargo.lock",), ("bun.lock",), ("Cargo.lock", "bun.lock")):
        with tempfile.TemporaryDirectory() as d:
            root = _tree(pathlib.Path(d))
            before = (root / "THIRD-PARTY-NOTICES").read_bytes()
            for lock in changed:
                _write(root / lock, "\n", mode="a")
            rc, out = _check_hashes(root)
            assert rc == 1 and gen().REGENERATE in out, (changed, rc, out)
            for lock in ("Cargo.lock", "bun.lock"):
                named = ("the tree's %s is %s" % (lock, _sha((root / lock).read_bytes()))) in out
                assert named is (lock in changed), (changed, lock, out)
            assert (root / "THIRD-PARTY-NOTICES").read_bytes() == before, "the check writes nothing"


def test_check_hashes_on_no_notices_file_or_no_lockfile() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _tree(pathlib.Path(d))
        (root / "THIRD-PARTY-NOTICES").unlink()
        rc, out = _check_hashes(root)
        assert rc == 1 and "THIRD-PARTY-NOTICES is stale: there is none" in out, (rc, out)
        assert gen().REGENERATE in out and not (root / "THIRD-PARTY-NOTICES").exists(), out
    for lock in ("Cargo.lock", "bun.lock"):
        with tempfile.TemporaryDirectory() as d:
            root = _tree(pathlib.Path(d))
            (root / lock).unlink()
            rc, out = _check_hashes(root)
            assert rc == 2 and out.startswith("gen_notices: cannot read") and lock in out, (lock, rc, out)


def test_check_hashes_reads_the_header_lines_packaging_reads() -> None:
    """The same lines as package-macos.sh's check: before the first rule, at column 0, with
    single spaces and lowercase hex, once each."""
    with tempfile.TemporaryDirectory() as d:
        root = _tree(pathlib.Path(d))
        cargo, bun = _sha(b"version = 4\n"), _sha(BUN_LOCK.encode())
        rule = "=" * 72
        cases = {  # label: (the notices' text, accepted)
            "current": ("X\n\nCargo.lock sha256 %s\nbun.lock sha256 %s\n%s\nbody\n" % (cargo, bun, rule), True),
            "a line planted after the rule": ("X\nCargo.lock sha256 %s\nbun.lock sha256 %s\n%s\nCargo.lock sha256 %s\n"
                                              % (cargo, bun, rule, cargo), True),
            "after the rule only": ("X\nbun.lock sha256 %s\n%s\nCargo.lock sha256 %s\n" % (bun, rule, cargo), False),
            "indented": ("X\n  Cargo.lock sha256 %s\nbun.lock sha256 %s\n" % (cargo, bun), False),
            "tab-separated": ("X\nCargo.lock\tsha256 %s\nbun.lock sha256 %s\n" % (cargo, bun), False),
            "doubled": ("X\nCargo.lock sha256 %s\nCargo.lock sha256 %s\nbun.lock sha256 %s\n" % (cargo, cargo, bun), False),
            "CRLF": ("X\nCargo.lock sha256 %s\r\nbun.lock sha256 %s\n" % (cargo, bun), False),
            "uppercase hex": ("X\nCargo.lock sha256 %s\nbun.lock sha256 %s\n" % (cargo.upper(), bun), False),
        }
        for label, (text, accepted) in cases.items():
            _write(root / "THIRD-PARTY-NOTICES", text)
            rc, out = _check_hashes(root)
            packaging, why = _refusal(root)
            assert (rc == 0) is accepted and (packaging == 0) is accepted, (label, rc, out, packaging, why)
            assert accepted or (rc == 1 and "Cargo.lock sha256" in out and "bun.lock sha256" not in out), (label, out)


def _refusal(root: pathlib.Path) -> tuple[int, str]:
    script = 'source "$1"; REPO_ROOT="$2"; require_current_notices && echo ACCEPTED'
    proc = subprocess.run(["bash", "-c", script, "bash", str(PACKAGE_MACOS), str(root)],
                          capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def test_package_macos_accepts_notices_that_record_the_trees_lockfiles() -> None:
    with tempfile.TemporaryDirectory() as d:
        rc, out = _refusal(_tree(pathlib.Path(d)))
        assert rc == 0 and "ACCEPTED" in out, (rc, out)


def test_package_macos_refuses_a_stale_lockfile_by_name() -> None:
    for lock in ("Cargo.lock", "bun.lock"):
        with tempfile.TemporaryDirectory() as d:
            root = _tree(pathlib.Path(d))
            _write(root / lock, "\n", mode="a")
            rc, out = _refusal(root)
            assert rc == 1 and "ACCEPTED" not in out, (rc, out)
            assert ("the tree's %s is %s" % (lock, _sha((root / lock).read_bytes()))) in out, out
            assert "gen_notices.py" in out, "the refusal names the command that regenerates the file"


def test_package_macos_refuses_missing_or_unreadable_notices() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _tree(pathlib.Path(d))
        (root / "THIRD-PARTY-NOTICES").unlink()
        rc, out = _refusal(root)
        assert rc == 1 and "THIRD-PARTY-NOTICES" in out and "ACCEPTED" not in out, (rc, out)
    with tempfile.TemporaryDirectory() as d:
        root = _tree(pathlib.Path(d))
        text = (root / "THIRD-PARTY-NOTICES").read_text(encoding="utf-8")
        _write(root / "THIRD-PARTY-NOTICES", text + text.splitlines()[2] + "\n")
        rc, out = _refusal(root)
        assert rc == 1 and 'single "Cargo.lock sha256' in out, (rc, out)
    if os.geteuid() != 0:  # root reads a mode-000 file
        with tempfile.TemporaryDirectory() as d:
            root = _tree(pathlib.Path(d))
            (root / "THIRD-PARTY-NOTICES").chmod(0)
            try:
                rc, out = _refusal(root)
            finally:
                (root / "THIRD-PARTY-NOTICES").chmod(0o644)
            assert rc == 1 and "Cannot read" in out and "ACCEPTED" not in out, (rc, out)


# A cut of deny.toml's [licenses] allow list, as the licence cases read it; OFL-1.1 is not in it.
ALLOWED = ["Apache-2.0", "Apache-2.0 WITH LLVM-exception", "MIT", "MPL-2.0"]
DENY = '[licenses]\nallow = ["Apache-2.0", "MIT", "MPL-2.0"]\n'


def test_an_spdx_expression_passes_by_or_and_with_and_parentheses() -> None:
    cases = {
        "MIT": True, "(MIT)": True, "((MIT))": True, "GPL-3.0-only": False,
        "Apache-2.0 OR MIT": True, "(MPL-2.0 OR Apache-2.0)": True, "GPL-3.0-only OR MIT": True,
        "MIT AND Apache-2.0": True, "MIT AND GPL-3.0-only": False,
        # AND binds tighter than OR; parentheses override that.
        "MIT OR GPL-3.0-only AND GPL-2.0-only": True, "GPL-3.0-only AND GPL-2.0-only OR MIT": True,
        "GPL-3.0-only AND (GPL-2.0-only OR MIT)": False, "(MIT OR GPL-3.0-only) AND Apache-2.0": True,
        "(MIT OR GPL-3.0-only) AND (GPL-2.0-only OR LGPL-2.1-only)": False,
        # `X WITH Y` passes only when `X WITH Y` itself is listed: X alone does not admit it.
        "Apache-2.0 WITH LLVM-exception": True, "Apache-2.0  WITH  LLVM-exception": True,
        "MIT WITH LLVM-exception": False, "Apache-2.0 WITH Classpath-exception-2.0": False,
        "GPL-2.0-only WITH LLVM-exception OR MIT": True,
        # An id matches whatever its case (SPDX's annex D.2); an operator only in capitals.
        "mit": True, "Mit OR GPL-3.0-only": True,
        # `+` and LicenseRef- are ids like any other: refused unless listed as written.
        "MIT+": False, "Apache-2.0+": False, "LicenseRef-MIT": False, "DocumentRef-x:LicenseRef-MIT": False,
        "UNLICENSED": False,
    }
    for expression, expected in cases.items():
        assert gen().spdx_satisfied(expression, ALLOWED) is expected, (expression, expected)
    assert gen().spdx_satisfied("Apache-2.0", ["Apache-2.0 WITH LLVM-exception"]) is False, "nor X WITH Y admit X"
    assert gen().spdx_satisfied("MIT+", ALLOWED + ["MIT+"]) is True
    assert gen().spdx_satisfied("LicenseRef-Fonts AND MIT", ALLOWED + ["LicenseRef-Fonts"]) is True


def test_a_package_ships_under_the_ids_of_the_branches_the_list_satisfies() -> None:
    """What a package with no licence file is given standard texts for: each satisfied
    alternative of an OR, every term of an AND, X and Y of `X WITH Y`, an id itself; spelled
    as the list spells them."""
    cases = {
        "MIT": ["MIT"], "(MIT)": ["MIT"], "mit": ["MIT"], "GPL-3.0-only": [],
        "MIT OR GPL-3.0-only": ["MIT"], "GPL-3.0-only OR mit": ["MIT"],
        "MIT OR Apache-2.0": ["Apache-2.0", "MIT"], "(MPL-2.0 OR Apache-2.0)": ["Apache-2.0", "MPL-2.0"],
        "MIT AND Apache-2.0": ["Apache-2.0", "MIT"], "MIT AND GPL-3.0-only": [],
        "MIT OR GPL-3.0-only AND GPL-2.0-only": ["MIT"], "(MIT OR GPL-3.0-only) AND Apache-2.0": ["Apache-2.0", "MIT"],
        "Apache-2.0 WITH LLVM-exception": ["Apache-2.0", "LLVM-exception"],
        "apache-2.0 WITH llvm-exception": ["Apache-2.0", "LLVM-exception"],
        "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT": ["Apache-2.0", "LLVM-exception", "MIT"],
        "GPL-2.0-only WITH LLVM-exception OR MIT": ["MIT"],
    }
    for expression, expected in cases.items():
        assert gen().spdx_allowed_ids(expression, ALLOWED) == expected, (expression, expected)
    for expression in ("", "MIT or Apache-2.0", "SEE LICENSE IN LICENSE.md"):
        try:
            ids = gen().spdx_allowed_ids(expression, ALLOWED)
        except ValueError:
            continue
        raise AssertionError("%r is no SPDX expression, yet gave ids: %r" % (expression, ids))


def test_an_expression_that_is_missing_or_not_spdx_is_refused_never_passed() -> None:
    deep = "(" * 5000 + "MIT" + ")" * 5000  # deeper than Python's recursion limit lets a parser go
    for expression in ("", "  ", "MIT OR", "OR MIT", "MIT AND AND Apache-2.0", "(MIT", "MIT)", "()",
                       "MIT Apache-2.0", "MIT or Apache-2.0", "MIT/X11", "MIT, Apache-2.0", "Apache 2.0",
                       "SEE LICENSE IN LICENSE.md", "MIT WITH", "WITH LLVM-exception",
                       "(Apache-2.0) WITH LLVM-exception", "MIT++", "a:b", deep):
        try:
            verdict = gen().spdx_satisfied(expression, ALLOWED + ["MIT Apache-2.0", "Apache 2.0"])
        except ValueError:
            continue
        raise AssertionError("%r is no SPDX expression, yet got a verdict: %r" % (expression, verdict))
    refused = gen().licence_refusals([("ok", "1.0.0", "MIT"), ("odd", "2.0.0", "MIT or Apache-2.0"),
                                      ("none", "3.0.0", ""), ("gpl", "4.0.0", "GPL-3.0-only")], ALLOWED)
    assert [line.split(":")[0] for line in refused] == ["odd 2.0.0", "none 3.0.0", "gpl 4.0.0"], refused
    assert "MIT or Apache-2.0" in refused[0] and "SPDX" in refused[0], refused[0]
    assert "no licence" in refused[1], refused[1]
    assert "GPL-3.0-only" in refused[2], refused[2]
    refused = gen().licence_refusals([("deep", "1.0.0", deep)], ALLOWED)
    assert len(refused) == 1 and refused[0].startswith("deep 1.0.0: (((") and "too deep" in refused[0], refused


def test_the_js_allow_list_is_deny_tomls_with_ofl_for_the_fonts() -> None:
    allowed = gen().js_allowed(DENY)
    assert sorted(allowed) == ["Apache-2.0", "MIT", "MPL-2.0", "OFL-1.1"], allowed
    assert gen().licence_refusals([("@fontsource/ibm-plex-sans", "5.3.0", "OFL-1.1")], allowed) == []
    assert len(gen().licence_refusals([("x", "1.0.0", "CC-BY-4.0")], allowed)) == 1
    try:
        gen().js_allowed("[licenses]\n")
    except gen().NoVerdict as e:
        assert "allow" in str(e), str(e)
    else:
        raise AssertionError("a deny.toml with no allow list gave a JS allow list")


def _generation_tree(root: pathlib.Path, licences: dict) -> str:
    """A tree the generator runs on end to end: both lockfiles, deny.toml and about.toml,
    the SPDX texts, node_modules holding the bun.lock closure (a, b 2.0.0, c, e) under
    `licences` and vite 8.3.5, the bundled tool, under MIT, and a stand-in cargo-about
    printing _about's JSON, whose path it returns."""
    _spdx(root)
    _write(root / "Cargo.lock", "version = 4\n")
    _write(root / "bun.lock", BUN_LOCK)
    _write(root / "deny.toml", DENY)
    _write(root / "scripts" / "release" / "about.toml", 'accepted = ["MIT", "Apache-2.0", "MPL-2.0"]\n')
    for name, version in (("a", "1.0.0"), ("b", "2.0.0"), ("c", "1.0.0"), ("e", "1.0.0")):
        _npm_package(root, name, version, licences[name], {"LICENSE": "%s's licence\n" % name})
    _vite(root)
    about = _write(root / "about.json", json.dumps(_about(root)))
    exe = _write(root / "cargo-about", '#!/bin/bash\nif [ "$1" = --version ]; then echo "cargo-about %s"; '
                 'else cat "%s"; fi\n' % (gen().CARGO_ABOUT_VERSION, about))
    exe.chmod(0o755)
    return str(exe)


def _generate(root: pathlib.Path, exe: str) -> tuple[int, str]:
    proc = subprocess.run([sys.executable, "-I", str(GEN), "--root", str(root), "--cargo-about", exe],
                          capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def _check_licences(root=None) -> tuple[int, str]:
    cmd = [sys.executable, "-I", str(GEN), "--check-licences"] + (["--root", str(root)] if root else [])
    proc = subprocess.run(cmd, capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def test_generation_refuses_a_js_package_the_list_does_not_allow_by_name_and_writes_nothing() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        exe = _generation_tree(root, {"a": "MIT", "b": "(MPL-2.0 OR Apache-2.0)", "c": "OFL-1.1", "e": "GPL-3.0-only"})
        rc, out = _generate(root, exe)
        assert rc == 2 and "e 1.0.0: GPL-3.0-only" in out, (rc, out)
        for allowed in ("a 1.0.0", "b 2.0.0", "c 1.0.0"):
            assert allowed not in out, (allowed, out)
        assert not (root / "THIRD-PARTY-NOTICES").exists(), "a refused generation writes nothing"


def test_generation_refuses_a_package_that_ships_no_licence_file_by_name_before_reading_a_text() -> None:
    # A package with no licence file is given the standard text of each id it ships under;
    # the refusal comes first, so a licence the list does not allow is refused by name.
    for expression in ("GPL-3.0-only", "MIT+", "MIT or Apache-2.0", "SEE LICENSE IN LICENSE.md"):
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            exe = _generation_tree(root, {"a": "MIT", "b": "MIT", "c": "MIT", "e": expression})
            (root / "node_modules" / ".bun" / "e@1.0.0" / "node_modules" / "e" / "LICENSE").unlink()
            rc, out = _generate(root, exe)
            assert rc == 2 and ("e 1.0.0: %s (" % expression) in out, (expression, rc, out)
            assert "standard text" not in out, (expression, out)
            assert not (root / "THIRD-PARTY-NOTICES").exists(), "a refused generation writes nothing"


def test_generation_refuses_a_js_package_that_names_no_licence() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        exe = _generation_tree(root, {"a": "MIT", "b": "MIT", "c": "MIT", "e": None})
        rc, out = _generate(root, exe)
        assert rc == 2 and "e 1.0.0" in out and "no licence" in out, (rc, out)
        assert not (root / "THIRD-PARTY-NOTICES").exists(), "a refused generation writes nothing"


def test_generation_accepts_the_fonts_ofl_and_the_file_it_writes_passes_the_licence_check() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        exe = _generation_tree(root, {"a": "MIT", "b": "(MPL-2.0 OR Apache-2.0)", "c": "OFL-1.1", "e": "Apache-2.0 OR MIT"})
        rc, out = _generate(root, exe)
        assert rc == 0, (rc, out)
        text = (root / "THIRD-PARTY-NOTICES").read_text(encoding="utf-8")
        assert gen().js_section(text) == [("a", "1.0.0", "MIT"), ("b", "2.0.0", "(MPL-2.0 OR Apache-2.0)"),
                                          ("c", "1.0.0", "OFL-1.1"), ("e", "1.0.0", "Apache-2.0 OR MIT"),
                                          ("vite", "8.3.5", "MIT")]
        rc, out = _check_licences(root)
        assert rc == 0 and "5 JavaScript packages" in out, (rc, out)


def test_generation_lists_vite_as_a_bundled_tool_with_its_own_notice_and_check_reads_it_back() -> None:
    """End to end: vite, a devDependency, is listed at the version bun.lock holds, given the
    part of its LICENSE.md before the bundled dependencies' sections, the bundled-tool line
    beside it, and counted; --check reads the file back as current."""
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        exe = _generation_tree(root, {"a": "MIT", "b": "MIT", "c": "MIT", "e": "MIT"})
        rc, out = _generate(root, exe)
        assert rc == 0, (rc, out)
        text = (root / "THIRD-PARTY-NOTICES").read_text(encoding="utf-8")
        assert "\nJavaScript packages: 5\n" in gen().header(text) + "\n", gen().header(text)
        assert "\nvite 8.3.5: MIT\n" in text, text
        js_texts = text[text.index("Licence texts: JavaScript packages"):]
        assert gen().normalize(VITE_NOTICE) in js_texts, js_texts
        assert "additionally contains" not in text and "Picomatch" not in text, js_texts
        assert _bundled_line(text, "vite", "8.3.5"), js_texts
        rc, out = _check(root, "--cargo-about", exe)
        assert rc == 0 and "is current" in out, (rc, out)


def _bundled_half(root: pathlib.Path, tools=None):
    """The JavaScript half of the fixture lock's closure (each package under MIT, with a
    licence file) and its bundled tools: gen_notices.BUNDLED_TOOLS unless `tools` is given."""
    doc = gen().load_bun_lock(_write(root / "bun.lock", BUN_LOCK))
    for name, version in (("a", "1.0.0"), ("b", "2.0.0"), ("c", "1.0.0"), ("e", "1.0.0")):
        _npm_package(root, name, version, "MIT", {"LICENSE": "%s's licence\n" % name})
    bundled = gen().bundled_keys(doc, "apps/openalpaca-gui", gen().BUNDLED_TOOLS if tools is None else tools)
    return gen().js_half(root, doc, gen().bun_closure(doc, "apps/openalpaca-gui"), ALLOWED, bundled)


def test_a_bundled_tool_outside_the_closure_is_listed_at_its_lock_version_with_its_notice_and_the_line() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        _vite(root)
        # Another vite installed beside it (an older install's): the lock's version is read.
        _npm_package(root, "vite", "8.3.4", "ISC", {"LICENSE.md": "an older vite\n"}, suffix="+9f9b3119fbf6d926")
        half = _bundled_half(root)
        assert half.packages == [("a", "1.0.0", "MIT"), ("b", "2.0.0", "MIT"), ("c", "1.0.0", "MIT"),
                                 ("e", "1.0.0", "MIT"), ("vite", "8.3.5", "MIT")], half.packages
        out = gen().render(["Cargo.lock sha256 0", "bun.lock sha256 0"], gen().Half(), half)
        assert "\nJavaScript packages: 5\n" in gen().header(out) + "\n", gen().header(out)
        assert "vite" in gen().header(out), "the header names the bundled tool"
        notice = gen().normalize(VITE_NOTICE)
        assert [t for t, g in half.groups.items() if ("vite", "8.3.5") in g.used_by] == [notice], half.groups
        assert notice in out and "additionally contains" not in out and "Picomatch" not in out, out
        assert "older" not in out, "the other installed vite is not read"
        assert _bundled_line(out, "vite", "8.3.5"), out
        assert not _bundled_line(out, "a", "1.0.0"), "a shipped package carries no bundled-tool line"


def test_a_bundled_tool_whose_licence_the_list_does_not_allow_is_refused_by_name() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        _vite(root, licence="GPL-3.0-only")
        try:
            _bundled_half(root)
        except gen().NoVerdict as e:
            assert "vite 8.3.5: GPL-3.0-only" in str(e) and "a 1.0.0" not in str(e), str(e)
        else:
            raise AssertionError("a bundled tool under GPL-3.0-only was listed")


def test_a_bundled_tool_the_lock_does_not_hold_is_refused_by_name() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = _spdx(pathlib.Path(d))
        _vite(root)
        tools = dict(gen().BUNDLED_TOOLS, nothere=gen().BUNDLED_TOOLS["vite"])
        try:
            _bundled_half(root, tools)
        except gen().NoVerdict as e:
            assert "nothere" in str(e) and "bun.lock" in str(e), str(e)
        else:
            raise AssertionError("a bundled tool bun.lock does not hold was listed")


def test_a_bundled_tools_licence_file_missing_or_without_its_cut_line_is_no_verdict() -> None:
    # The tool's own notice is read from the file it names, cut where that file's own form
    # says; a file gone or a form that changed is named, never guessed or substituted (a
    # standard text would drop the tool's copyright line).
    for label, files in (("no cut line", {"LICENSE.md": VITE_LICENCE.replace("# Licenses of bundled dependencies",
                                                                             "# Bundled")}),
                         ("no file", {"LICENSE": VITE_NOTICE})):
        with tempfile.TemporaryDirectory() as d:
            root = _spdx(pathlib.Path(d))
            _vite(root, files=files)
            try:
                _bundled_half(root)
            except gen().NoVerdict as e:
                assert "vite 8.3.5" in str(e) and "LICENSE.md" in str(e), (label, str(e))
                assert "# Licenses of bundled dependencies" in str(e), (label, str(e))
            else:
                raise AssertionError("%s: vite was listed without its own notice" % label)


def _no_licence_file(root: pathlib.Path, name: str, version: str) -> None:
    (root / "node_modules" / ".bun" / ("%s@%s" % (name, version)) / "node_modules" / name / "LICENSE").unlink()


def test_generation_lists_and_marks_a_package_with_no_licence_file_and_no_vendored_text() -> None:
    """The owner's ruling of 2026-11-01, "make sure that if a license text of package is
    missing, it is still fine": OFL-1.1 is allowed, the fixture vendors no text for it, and
    e ships no licence file. The file is written, e listed with its expression and marked,
    the header counting it; --check reads it back as current and --check-licences passes it."""
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        exe = _generation_tree(root, {"a": "MIT", "b": "MIT", "c": "MIT", "e": "OFL-1.1"})
        _no_licence_file(root, "e", "1.0.0")
        rc, out = _generate(root, exe)
        assert rc == 0, (rc, out)
        text = (root / "THIRD-PARTY-NOTICES").read_text(encoding="utf-8")
        for line in ("Rust crates given a substituted text: 1", "Rust crates missing a licence text: 0",
                     "JavaScript packages given a substituted text: 0", "JavaScript packages missing a licence text: 1"):
            assert ("\n%s\n" % line) in gen().header(text) + "\n", (line, gen().header(text))
        rule = "-" * 72
        assert ("\n%s\nOFL-1.1: no licence text available\n"
                "  e 1.0.0 (ships no licence file; no standard text available)\n%s\n" % (rule, rule)) in text, text
        assert gen().js_section(text) == [("a", "1.0.0", "MIT"), ("b", "2.0.0", "MIT"), ("c", "1.0.0", "MIT"),
                                          ("e", "1.0.0", "OFL-1.1"), ("vite", "8.3.5", "MIT")]
        rc, out = _check(root, "--cargo-about", exe)
        assert rc == 0 and "is current" in out, (rc, out)
        rc, out = _check_licences(root)
        assert rc == 0 and "5 JavaScript packages" in out, (rc, out)
        rc, out = _generate(root, exe)
        assert rc == 0 and (root / "THIRD-PARTY-NOTICES").read_text(encoding="utf-8") == text, "the same bytes again"


def test_generation_gives_a_package_with_no_licence_file_the_text_of_its_allowed_alternative_alone() -> None:
    """`MIT OR GPL-3.0-only` with no licence file: the package ships under MIT, the
    alternative the list allows, so MIT's text is substituted and GPL-3.0-only's, which
    no one vendored, is neither looked for nor missed."""
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        exe = _generation_tree(root, {"a": "MIT", "b": "MIT", "c": "MIT", "e": "MIT OR GPL-3.0-only"})
        _no_licence_file(root, "e", "1.0.0")
        rc, out = _generate(root, exe)
        assert rc == 0, (rc, out)
        text = (root / "THIRD-PARTY-NOTICES").read_text(encoding="utf-8")
        assert "\ne 1.0.0: MIT OR GPL-3.0-only\n" in text, text
        js_texts = text[text.index("Licence texts: JavaScript packages"):]
        assert "MIT, the SPDX standard text (substituted)\n  e 1.0.0 (ships no licence file)\n" in js_texts, js_texts
        assert "GPL" not in js_texts and "no licence text available" not in text, js_texts
        for line in ("JavaScript packages given a substituted text: 1", "JavaScript packages missing a licence text: 0"):
            assert ("\n%s\n" % line) in text, (line, gen().header(text))


def test_apache_with_the_llvm_exception_gets_both_vendored_texts() -> None:
    """`Apache-2.0 WITH LLVM-exception` with no licence file: Apache-2.0's text and the
    exception's, both read from the repository's vendored texts."""
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        vendored = SCRIPTS / "release" / "spdx"
        for ident in ("Apache-2.0", "LLVM-exception"):
            _write(root / "scripts" / "release" / "spdx" / ("%s.txt" % ident),
                   (vendored / ("%s.txt" % ident)).read_text(encoding="utf-8"))
        doc = gen().load_bun_lock(_write(root / "bun.lock", BUN_LOCK))
        for name, version in (("a", "1.0.0"), ("c", "1.0.0"), ("e", "1.0.0")):
            _npm_package(root, name, version, "Apache-2.0", {"LICENSE": "%s's licence\n" % name})
        _npm_package(root, "b", "2.0.0", "Apache-2.0 WITH LLVM-exception", {})
        half = gen().js_half(root, doc, gen().bun_closure(doc, "apps/openalpaca-gui"), ALLOWED)
        substituted = {label: text for text, g in half.groups.items() if g.substituted for label in g.labels}
        assert sorted(substituted) == ["Apache-2.0, the SPDX standard text (substituted)",
                                       "LLVM-exception, the SPDX standard text (substituted)"], sorted(substituted)
        exception = substituted["LLVM-exception, the SPDX standard text (substituted)"]
        assert exception.startswith("---- LLVM Exceptions to the Apache 2.0 License ----\n"), exception[:80]
        for text in substituted.values():
            assert half.groups[text].substituted == {("b", "2.0.0")}, half.groups[text].substituted


def test_generation_lists_and_marks_a_crate_cargo_about_names_no_licence_for() -> None:
    """End to end through the stand-in cargo-about: deny.toml and about.toml accept
    LicenseRef-Foo, and the JSON holds a crate under it that ships no licence file and that
    no licence names. The file is written, the crate listed and marked, the header counting
    it, and --check reads it back as current."""
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        exe = _generation_tree(root, {"a": "MIT", "b": "MIT", "c": "MIT", "e": "MIT"})
        allow = '["Apache-2.0", "MIT", "MPL-2.0", "LicenseRef-Foo"]'
        _write(root / "deny.toml", "[licenses]\nallow = %s\n" % allow)
        _write(root / "scripts" / "release" / "about.toml", "accepted = %s\n" % allow)
        about = _about(root)
        about["crates"].append(_ref_crate(root, "refonly", "1.0.0", "LicenseRef-Foo", {}))
        _write(root / "about.json", json.dumps(about))
        rc, out = _generate(root, exe)
        assert rc == 0, (rc, out)
        text = (root / "THIRD-PARTY-NOTICES").read_text(encoding="utf-8")
        assert "\nrefonly 1.0.0: LicenseRef-Foo\n" in text, text
        for line in ("Rust crates given a substituted text: 1", "Rust crates missing a licence text: 1"):
            assert ("\n%s\n" % line) in gen().header(text) + "\n", (line, gen().header(text))
        rule = "-" * 72
        assert ("\n%s\nLicenseRef-Foo: no licence text available\n"
                "  refonly 1.0.0 (ships no licence file; no standard text available)\n%s\n" % (rule, rule)) in text, text
        rc, out = _check(root, "--cargo-about", exe)
        assert rc == 0 and "is current" in out, (rc, out)


def _notices(js: list, rust: list) -> str:
    rust_half, js_half = gen().Half(), gen().Half()
    rust_half.packages, js_half.packages = list(rust), list(js)
    return gen().render(["Cargo.lock sha256 0", "bun.lock sha256 0"], rust_half, js_half)


JS = [("@fontsource/ibm-plex-sans", "5.3.0", "OFL-1.1"), ("dompurify", "3.4.17", "(MPL-2.0 OR Apache-2.0)"),
      ("react", "19.3.1", "MIT")]
# Rust lines outside the list (B-22's body names 13 such declared expressions): not read.
RUST = [("lgpl-only", "1.0.0", "LGPL-2.1-or-later"), ("r-efi", "5.3.0", "MIT OR Apache-2.0 OR LGPL-2.1-or-later")]


def test_check_licences_reads_the_files_js_lines_alone_and_names_each_refused_package() -> None:
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        _write(root / "deny.toml", DENY)
        _write(root / "THIRD-PARTY-NOTICES", _notices(JS, RUST))
        rc, out = _check_licences(root)
        assert rc == 0 and "3 JavaScript packages" in out and "lgpl-only" not in out, (rc, out)
        _write(root / "THIRD-PARTY-NOTICES", _notices(JS + [("left-pad", "1.3.0", "WTFPL"), ("nolic", "0.1.0", "")], RUST))
        rc, out = _check_licences(root)
        assert rc == 1 and "left-pad 1.3.0: WTFPL" in out and "nolic 0.1.0" in out and "no licence" in out, (rc, out)
        assert "react" not in out and "dompurify" not in out and "lgpl-only" not in out, out


def test_check_licences_gives_no_verdict_on_a_file_it_cannot_read_whole() -> None:
    whole = _notices(JS, RUST)
    cases = {  # label: (the file, or None for no file; what the refusal names)
        "no JavaScript section": (whole.replace("\nJavaScript packages\n", "\nJS packages\n"), "section"),
        "a line fewer than the header counts": (whole.replace("react 19.3.1: MIT\n", ""), "counts 3"),
        "a line that is not `name version: licence`": (whole.replace("react 19.3.1: MIT\n", "react: MIT\n"),
                                                        "'react: MIT'"),
        "no file": (None, "THIRD-PARTY-NOTICES"),
    }
    for label, (text, named) in cases.items():
        with tempfile.TemporaryDirectory() as d:
            root = pathlib.Path(d)
            _write(root / "deny.toml", DENY)
            if text is not None:
                _write(root / "THIRD-PARTY-NOTICES", text)
            rc, out = _check_licences(root)
            assert rc == 2 and out.startswith("gen_notices: ") and named in out, (label, rc, out)
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        _write(root / "deny.toml", "[licenses]\n")
        _write(root / "THIRD-PARTY-NOTICES", whole)
        rc, out = _check_licences(root)
        assert rc == 2 and out.startswith("gen_notices: ") and "deny.toml" in out, (rc, out)


def test_the_committed_notices_js_licences_pass() -> None:
    """The repository's own THIRD-PARTY-NOTICES and deny.toml, read as CI's step reads them."""
    rc, out = _check_licences()
    assert rc == 0 and "JavaScript packages" in out, (rc, out)


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
