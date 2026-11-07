#!/usr/bin/env python3.11
"""THIRD-PARTY-NOTICES from Cargo.lock and bun.lock (B-22): every package the release ships,
with its licence text, and the hash of each lockfile it was read from.

Usage:
  python3.11 -I scripts/release/gen_notices.py [--check | --check-hashes | --check-licences] [--root DIR]
                                               [--cargo-about EXE]

Writes THIRD-PARTY-NOTICES at the repository root (ROOT, default: this file's
grandparent's parent). Run it after `bun install --frozen-lockfile`, from a tree whose
lockfiles are the ones to ship, and commit the file with them: package-macos.sh refuses
to package while either hash it records is not the tree's lockfile's. Never edit the file
by hand. With --check it writes nothing: when a recorded hash is not the tree's
lockfile's it names that lockfile (exit 1) without regenerating; else it regenerates in
memory and compares. With --check-hashes it stops after that first step, reading only the
committed file's header and the two lockfiles (no install, no cargo-about), the local
gate a lock-moving commit runs: it sees a lockfile moved without the file regenerated,
not a change to anything else the text is generated from (node_modules, about.toml, the
vendored texts, this file), which --check sees. With --check-licences it reads only the
committed file's JavaScript lines and deny.toml (no install, no cargo-about), a local
gate too, and checks each package's licence as generation does (below).

What it lists:
  Rust        the crates the daemon (openalpacad), the CLI (openalpaca) and the app
              (openalpaca_gui) link, as cargo-about 0.9.2 reads the workspace
              (`--workspace --all-features --frozen --fail`, every target) with
              scripts/release/about.toml: normal dependencies only, the workspace's own
              unpublished crates left out. cargo-about chooses each crate's licence from
              about.toml's `accepted` list, which must be deny.toml's [licenses] allow list
              (the order is cargo-about's preference among the alternatives of an OR).
              Its text counts as read from the crate only when cargo-about took it from a
              licence file (by name, below; not a source file whose header it matched)
              that is the crate's own: in the crate's directory, or, as cargo-about gives
              a text several crates share one crate's path, equal to one of the crate's
              own licence files. For a licence with no such text the crate's own licence
              files are given as shipped, and where the crate ships none, the standard
              text below. A crate cargo-about names no licence for (0.9.2 names no
              LicenseRef- id it has no file for) is read as a JavaScript package is,
              by the branches of its expression the list satisfies. Install it with:
                cargo install --locked --version 0.9.2 --features cli \
                  --root ~/.cache/openalpaca-deps-tools cargo-about
              (no other version is run).
  JavaScript  the packages the app's frontend ships: the closure, in bun.lock, of
              apps/openalpaca-gui/package.json's dependencies through each package's
              dependencies, optionalDependencies and peerDependencies the lock resolves
              (devDependencies never), with every licence file each one ships in
              node_modules; one that ships none gets the standard text of each licence
              (and exception) it ships under: those of the branches of the expression
              its package.json declares that the list below satisfies (each allowed
              alternative of an OR, every term of an AND, both ids of `X WITH Y`).
              `bun pm licenses` gives no licence text and does not tell a shipped
              package from a development one, so it is not used.
  Bundled     the build tools whose own runtime code the bundler writes into the app's
  tools       bundle (BUNDLED_TOOLS: vite, whose module-preload polyfill and preload
              helper the bundle carries): devDependencies, so no package of that
              closure. Each is read at the version bun.lock resolves the workspace's
              name to (a tool the lock does not hold is no verdict), its licence checked
              as a shipped package's (below), and listed among the JavaScript packages,
              counted with them; its text is its own notice, the lines of the licence
              file it names before the line it names (vite's LICENSE.md before "# Licenses
              of bundled dependencies": what follows covers what vite bundles into its
              own build, not code it writes into ours), given beside a line calling it a
              build tool whose runtime code the bundle carries. That file gone, or that
              line not in it once, is no verdict, never a substituted text, which would
              drop the tool's own copyright line. A bundled tool the closure holds too is
              listed once, as the shipped package it is.
  Licence files are a package directory's files named LICENSE*, LICENCE*, COPYING*,
  COPYRIGHT* or NOTICE* (any case), and every file in a licence directory there
  (LICENSES/, .licenses/, any case), bar an SPDX document (*.spdx). A standard text is
  spdx/license-list-data's at tag v3.29.0, vendored under scripts/release/spdx/ for every
  id deny.toml's allow list and JS_ALSO_ALLOWED name, and marked as substituted. A text
  missing never stops the notices (the owner's ruling of 2026-11-01): for an id with no
  text vendored (by its exact file name) the package is listed all the same, with its
  expression, under an entry "<id>: no licence text available" that marks it, and the
  header counts it. What stops them is a licence missing or not allowed, never its text:
  a package.json that names none or one the list does not allow (below); cargo-about
  failing on a crate it finds no licence for or one the list does not satisfy; a crate
  it names none for whose expression the list does not satisfy. Each text is given once,
  its lines' trailing whitespace and CRLFs gone, after the packages it covers; everything
  is sorted and nothing is dated, so the same tree writes the same bytes.

The JavaScript licences: each package's, the SPDX expression its package.json declares,
must be satisfied by deny.toml's [licenses] allow list (the one cargo-about's `accepted`
is held to for the Rust half) or by JS_ALSO_ALLOWED below, which is the JavaScript half's
alone; else generation refuses, before it reads any licence text, as cargo-about's --fail
refuses a crate, and writes nothing, naming each package, its version and its expression.
By SPDX's rules (2.3, annex D): A OR B passes when either does, A AND B when both do, AND
binding before OR and parentheses grouping; `X WITH Y` only when `X WITH Y` itself is
listed; an id matches in any case, an operator only in capitals, and `+` and LicenseRef-
ids only as listed; an id is any SPDX idstring, not looked up in SPDX's licence list; a
missing or unparseable expression is refused, never passed. The Rust lines are not read
by this check: cargo-about's --fail holds the Rust half at generation, and cargo-deny,
run by hand on deny.toml, holds Cargo.lock to the same list.

Exit: 0 written, current under --check, the tree's hashes recorded under --check-hashes,
or every JavaScript licence allowed under --check-licences; 1 stale under --check or
--check-hashes, or a JavaScript licence refused under --check-licences; 2 nothing written
or checked: a usage error, a JavaScript licence refused at generation, or no verdict (a
lockfile, node_modules, deny.toml or the committed file that cannot be read, a bundled
tool bun.lock does not hold or whose notice cannot be read as BUNDLED_TOOLS says,
cargo-about missing, another version or failing).
"""

import dataclasses
import functools
import hashlib
import importlib.util
import itertools
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]

NOTICES = "THIRD-PARTY-NOTICES"
LOCKS = ("Cargo.lock", "bun.lock")
CARGO_ABOUT_VERSION = "0.9.2"
CARGO_ABOUT_TOOLS = pathlib.Path.home() / ".cache" / "openalpaca-deps-tools" / "bin" / "cargo-about"
ABOUT_TOML = "scripts/release/about.toml"
SPDX_DIR = "scripts/release/spdx"
SPDX_TAG = "v3.29.0"
GUI_WORKSPACE = "apps/openalpaca-gui"
# bun.lock's reader: lockdates.py's (C5), whose trailing-comma pass and known versions are reused.
LOCKDATES = "tasks/deps-upgrade/tools/gbase/lockdates.py"
REGENERATE = "python3.11 -I scripts/release/gen_notices.py"
INSTALL = ("cargo install --locked --version %s --features cli --root ~/.cache/openalpaca-deps-tools cargo-about"
           % CARGO_ABOUT_VERSION)

LICENCE_FILE = re.compile(r"(?i)^(licen[cs]e|copying|copyright|notice)")
LICENCE_DIR = re.compile(r"(?i)^\.?licen[cs]es$")
# The licences the JavaScript half may ship beside deny.toml's allow list, each with its
# reason. Never added to deny.toml, which is the Rust half's policy too.
JS_ALSO_ALLOWED = {
    "OFL-1.1": "the two @fontsource fonts (IBM Plex): it lets a font be bundled with software and asks"
               " that its text go with it, which this file gives",
}
JS_TITLE = "JavaScript packages"
JS_COUNT = re.compile(r"^%s: (\d+)$" % JS_TITLE, re.M)
PACKAGE_LINE = re.compile(r"^(\S+) (\S+):(?: (.*))?$")
SPDX_ID = re.compile(r"[A-Za-z0-9.-]+\+?|(?:DocumentRef-[A-Za-z0-9.-]+:)?LicenseRef-[A-Za-z0-9.-]+")
SPDX_EXCEPTION = re.compile(r"[A-Za-z0-9.-]+")
HASH_LINE = re.compile(r"^(%s) sha256 ([0-9a-f]{64})$" % "|".join(re.escape(lock) for lock in LOCKS), re.M)
SPDX_OPERATORS = {"AND", "OR", "WITH"}
RULE = "=" * 72
THIN = "-" * 72


class NoVerdict(Exception):
    pass


@dataclasses.dataclass(frozen=True)
class BundledTool:
    """A build tool whose own runtime code the bundler writes into the app's bundle: what of
    it the bundle carries, the licence file its notice is read from, and the line of that
    file its own notice ends before (what follows covers what the tool bundles into its own
    build, not code it writes into ours)."""
    carries: str
    licence_file: str
    ends_before: str


# The bundled tools (B-22's (d), taken 2026-11-06): devDependencies, so no package of the
# JavaScript closure, whose runtime code the app's bundle carries all the same.
BUNDLED_TOOLS = {
    "vite": BundledTool(carries="its module-preload polyfill and preload helper", licence_file="LICENSE.md",
                        ends_before="# Licenses of bundled dependencies"),
}


@dataclasses.dataclass
class Group:
    """One licence text: what it is, and the packages it covers (substituted: given the
    standard text because they ship no licence file)."""
    labels: set = dataclasses.field(default_factory=set)
    used_by: set = dataclasses.field(default_factory=set)
    substituted: set = dataclasses.field(default_factory=set)


@dataclasses.dataclass
class Half:
    """One ecosystem: its packages as (name, version, licence), its texts by content, the
    ids it has no text for, each with the packages that ship no licence file under it, and
    its bundled tools, (name, version): BundledTool."""
    packages: list = dataclasses.field(default_factory=list)
    groups: dict = dataclasses.field(default_factory=dict)
    missing: dict = dataclasses.field(default_factory=dict)
    bundled: dict = dataclasses.field(default_factory=dict)

    def add(self, text: str, label: str, package: tuple, substituted: bool = False) -> None:
        group = self.groups.setdefault(normalize(text), Group())
        group.labels.add(label)
        (group.substituted if substituted else group.used_by).add(package)

    def standard(self, root: pathlib.Path, ident: str, package: tuple) -> None:
        """For a package that ships no licence file, the standard text of `ident`, marked
        substituted; with none vendored, the package recorded as missing it, never refused."""
        text = spdx_text(root, ident)
        if text is None:
            self.missing.setdefault(ident, set()).add(package)
        else:
            self.add(text, "%s, the SPDX standard text (substituted)" % ident, package, True)

    def substituted_count(self) -> int:
        return len(set().union(*(g.substituted for g in self.groups.values())))

    def missing_count(self) -> int:
        return len(set().union(*self.missing.values()))


# ------------------------------------------------------------------ text
def normalize(text: str) -> str:
    """The text with no BOM, LF line ends, no trailing whitespace and no blank edge lines."""
    text = text.lstrip("﻿").replace("\r\n", "\n").replace("\r", "\n")
    lines = [line.rstrip() for line in text.split("\n")]
    while lines and not lines[0]:
        lines.pop(0)
    while lines and not lines[-1]:
        lines.pop()
    return "\n".join(lines) + "\n"


def read_text(path: pathlib.Path) -> str:
    try:
        return path.read_bytes().decode("utf-8", errors="replace")
    except OSError as e:
        raise NoVerdict("cannot read %s: %s" % (path, e))


def listing(directory: pathlib.Path) -> list:
    try:
        return sorted(directory.iterdir(), key=lambda p: p.name)
    except OSError as e:
        raise NoVerdict("cannot list %s: %s" % (directory, e))


def licence_files(directory: pathlib.Path) -> list:
    """[(name, text)] of a package's licence files, by name: those directly in its
    directory, and every file in a licence directory there (named `dir/file`)."""
    found = []
    for p in listing(directory):
        if p.is_dir() and LICENCE_DIR.match(p.name):
            found += [("%s/%s" % (p.name, q.name), q) for q in listing(p) if q.is_file()]
        elif p.is_file() and LICENCE_FILE.match(p.name):
            found.append((p.name, p))
    return [(name, read_text(p)) for name, p in sorted(found) if not name.lower().endswith(".spdx")]


def spdx_text(root: pathlib.Path, ident: str) -> str | None:
    """The vendored standard text of a licence or exception id, or None when there is none:
    found by its exact file name, so a case-insensitive file system reads the tree as any
    other does."""
    directory = root / SPDX_DIR
    try:
        names = {p.name for p in directory.iterdir()}
    except FileNotFoundError:
        names = set()
    except OSError as e:
        raise NoVerdict("cannot list %s: %s" % (directory, e))
    name = "%s.txt" % ident
    return read_text(directory / name) if name in names else None


# ------------------------------------------------------------------ lockfile hashes
def sha256_file(path: pathlib.Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as e:
        raise NoVerdict("cannot read %s: %s" % (path, e))


def hash_lines(root: pathlib.Path) -> list:
    return ["%s sha256 %s" % (lock, sha256_file(root / lock)) for lock in LOCKS]


def header(text: str) -> str:
    """The notices' header, where the hash lines are: every line before the first rule
    (package-macos.sh reads the same lines)."""
    lines = text.split("\n")
    return "\n".join(lines[:lines.index(RULE)] if RULE in lines else lines)


def recorded_hashes(text: str) -> dict:
    return {m.group(1): m.group(2) for m in HASH_LINE.finditer(header(text))}


def stale_locks(root: pathlib.Path, text: str) -> list:
    """One line per lockfile whose hash the notices' header does not record once, as the
    tree's."""
    out = []
    for lock in LOCKS:
        found = [m.group(2) for m in HASH_LINE.finditer(header(text)) if m.group(1) == lock]
        actual = sha256_file(root / lock)
        if len(found) != 1:
            out.append("%s is stale: it records no single \"%s sha256 <hex>\" line" % (NOTICES, lock))
        elif found[0] != actual:
            out.append("%s is stale: it records %s sha256 %s, but the tree's %s is %s"
                       % (NOTICES, lock, found[0], lock, actual))
    return out


# ------------------------------------------------------------------ Rust: cargo-about
def deny_allow(deny_text: str) -> list:
    """deny.toml's [licenses] allow list (P34's): the licences both halves are held to."""
    try:
        licences = tomllib.loads(deny_text).get("licenses")
    except tomllib.TOMLDecodeError as e:
        raise NoVerdict("deny.toml is not TOML: %s" % e)
    allow = licences.get("allow") if isinstance(licences, dict) else None
    if not isinstance(allow, list) or not all(isinstance(a, str) for a in allow):
        raise NoVerdict("deny.toml has no [licenses] allow list of licence names")
    return allow


def check_accepted(about_text: str, deny_text: str) -> None:
    """Refuses an about.toml whose `accepted` set is not deny.toml's [licenses] allow set."""
    try:
        accepted = tomllib.loads(about_text).get("accepted")
    except tomllib.TOMLDecodeError as e:
        raise NoVerdict("%s is not TOML: %s" % (ABOUT_TOML, e))
    if not isinstance(accepted, list):
        raise NoVerdict("%s has no `accepted` list" % ABOUT_TOML)
    allow = deny_allow(deny_text)
    if set(accepted) != set(allow):
        raise NoVerdict("%s's accepted licences are not deny.toml's allow list: only in %s: %s; only in deny.toml: %s"
                        % (ABOUT_TOML, ABOUT_TOML, sorted(set(accepted) - set(allow)) or "none",
                           sorted(set(allow) - set(accepted)) or "none"))


def find_cargo_about(given) -> str:
    if given:
        return given
    found = shutil.which("cargo-about")
    if found:
        return found
    if CARGO_ABOUT_TOOLS.is_file():
        return str(CARGO_ABOUT_TOOLS)
    raise NoVerdict("cargo-about not found on PATH or at %s; install it: %s" % (CARGO_ABOUT_TOOLS, INSTALL))


def run_cargo_about(root: pathlib.Path, exe: str) -> dict:
    try:
        version = subprocess.run([exe, "--version"], capture_output=True, text=True).stdout.strip()
    except OSError as e:
        raise NoVerdict("cannot run %s: %s" % (exe, e))
    if version != "cargo-about %s" % CARGO_ABOUT_VERSION:
        raise NoVerdict("%s is %r, not cargo-about %s, the version this file is generated with; install it: %s"
                        % (exe, version, CARGO_ABOUT_VERSION, INSTALL))
    cmd = [exe, "generate", "--format", "json", "--frozen", "--all-features", "--workspace", "--fail",
           "-c", str(root / ABOUT_TOML), "-m", str(root / "Cargo.toml")]
    proc = subprocess.run(cmd, capture_output=True, text=True, cwd=root)
    if proc.stderr:
        sys.stderr.write(proc.stderr)
    if proc.returncode != 0:
        raise NoVerdict("cargo-about failed (exit %d): %s" % (proc.returncode, " ".join(cmd)))
    try:
        return json.loads(proc.stdout)
    except ValueError as e:
        raise NoVerdict("cargo-about wrote no JSON: %s" % e)


def read_from_crate(source, text: str, directory: pathlib.Path, own_texts) -> bool:
    """Whether cargo-about read this text from the crate's own licence file: one named as a
    licence file (not a source file whose licence header it matched), in the crate's
    directory or, since cargo-about gives a text several crates share one crate's path,
    one of `own_texts()`, the crate's licence files' normalized texts."""
    if not source:
        return False
    path = pathlib.Path(source)
    if not LICENCE_FILE.match(path.name) or path.name.lower().endswith(".spdx"):
        return False
    return path.is_relative_to(directory) or normalize(text) in own_texts()


def rust_reading(about: dict) -> tuple:
    """({crate: the licence ids cargo-about gave it}, {crate: [(id, text)] read from its
    own licence files}), each crate as (name, version)."""
    dirs = {(c["package"]["name"], c["package"]["version"]): pathlib.Path(c["package"]["manifest_path"]).parent
            for c in about.get("crates", [])}

    @functools.cache
    def own_texts(key):
        return frozenset(normalize(text) for _, text in licence_files(dirs[key]))

    named, read = {}, {}
    for lic in about.get("licenses", []):
        for use in lic.get("used_by", []):
            key = (use["crate"]["name"], use["crate"]["version"])
            named.setdefault(key, set()).add(lic["id"])
            if key in dirs and read_from_crate(lic.get("source_path"), lic["text"], dirs[key],
                                               functools.partial(own_texts, key)):
                read.setdefault(key, []).append((lic["id"], lic["text"]))
    return named, read


def rust_half(root: pathlib.Path, about: dict, accepted) -> Half:
    """The crates of cargo-about's JSON and their texts: each licence it read from the
    crate's own licence file; for one it read none for, the crate's own licence files, or
    with none the standard text. cargo-about's own fallback text, a source file's header
    and every path stay out. A crate cargo-about names no licence for (0.9.2 names no
    LicenseRef- id it has no file for) ships under the ids of the branches of its
    expression the accepted licences satisfy, as a JavaScript package does; refused only
    when there are none. Where cargo-about names other ids, what its JSON leaves out is
    neither given nor marked: the exception of `X WITH Y` (it names X) and a LicenseRef-
    id beside them."""
    half = Half()
    named, read = rust_reading(about)
    for krate in about.get("crates", []):
        pkg = krate["package"]
        key = (pkg["name"], pkg["version"])
        licence = " ".join(str(pkg.get("license") or krate.get("license")).split())
        half.packages.append((pkg["name"], pkg["version"], licence))
        ids = named.get(key)
        if ids is None:
            try:
                ids = set(spdx_allowed_ids(licence, accepted))
            except ValueError:
                ids = set()
            if not ids:
                raise NoVerdict("cargo-about gave %s %s no licence" % key)
        for ident, text in read.get(key, []):
            half.add(text, ident, key)
        unread = ids - {ident for ident, _ in read.get(key, [])}
        if unread:
            shipped = licence_files(pathlib.Path(pkg["manifest_path"]).parent)
            for name, text in shipped:
                half.add(text, "%s as shipped" % name, key)
            if not shipped:
                for ident in sorted(unread):
                    half.standard(root, ident, key)
    half.packages.sort(key=package_key)
    return half


# ------------------------------------------------------------------ JavaScript: bun.lock
def lockdates():
    """lockdates.py, loaded from this repository by its path (it is no package)."""
    path = ROOT / LOCKDATES
    spec = importlib.util.spec_from_file_location("lockdates", path)
    if spec is None or spec.loader is None or not path.is_file():
        raise NoVerdict("cannot load bun.lock's reader %s" % path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_bun_lock(path: pathlib.Path) -> dict:
    reader = lockdates()
    try:
        doc = json.loads(reader.strip_trailing_commas(read_text(path)))
    except ValueError as e:
        raise NoVerdict("%s is not a bun.lock (not JSON once its trailing commas go: %s)" % (path, e))
    if not isinstance(doc, dict) or not isinstance(doc.get("packages"), dict) or not isinstance(doc.get("workspaces"), dict):
        raise NoVerdict("%s is not a bun.lock: no `packages` or `workspaces` block" % path)
    if doc.get("lockfileVersion") not in reader.BUN_LOCK_VERSIONS:
        raise NoVerdict("%s: lockfileVersion %r — this reader knows %s"
                        % (path, doc.get("lockfileVersion"), reader.BUN_LOCK_VERSIONS))
    return doc


def key_segments(key: str) -> list:
    """bun.lock's package key `a/@s/b/c` as its path of names: ['a', '@s/b', 'c']."""
    parts, out = key.split("/"), []
    while parts:
        head = parts.pop(0)
        out.append(head + "/" + parts.pop(0) if head.startswith("@") and parts else head)
    return out


def resolve(packages: dict, parent: list, name: str):
    """The key a dependency `name` of the package at `parent` resolves to: the nearest of
    parent/name, its parent's, …, name — Bun's nesting — or None."""
    path = list(parent)
    while True:
        key = "/".join(path + [name])
        if key in packages:
            return key
        if not path:
            return None
        path.pop()


def entry_meta(entry) -> dict:
    return entry[2] if isinstance(entry, list) and len(entry) > 2 and isinstance(entry[2], dict) else {}


def bun_closure(doc: dict, workspace: str) -> list:
    """The package keys the workspace's dependencies reach, sorted. An optional dependency
    or peer the lock does not hold is skipped; a required one it does not hold is refused."""
    packages = doc["packages"]
    ws = doc["workspaces"].get(workspace)
    if not isinstance(ws, dict):
        raise NoVerdict("bun.lock has no workspace %s" % workspace)
    stack = [([ws.get("name", workspace)], name, False) for name in ws.get("dependencies", {})]
    stack += [([ws.get("name", workspace)], name, True) for name in ws.get("optionalDependencies", {})]
    seen = set()
    while stack:
        parent, name, optional = stack.pop()
        key = resolve(packages, parent, name)
        if key is None:
            if optional:
                continue
            raise NoVerdict("bun.lock: %s needs %s, which the lock does not hold" % ("/".join(parent), name))
        if key in seen:
            continue
        seen.add(key)
        meta, here = entry_meta(packages[key]), key_segments(key)
        optional_peers = set(meta.get("optionalPeers", []))
        stack += [(here, n, False) for n in meta.get("dependencies", {})]
        stack += [(here, n, True) for n in meta.get("optionalDependencies", {})]
        stack += [(here, n, n in optional_peers) for n in meta.get("peerDependencies", {})]
    return sorted(seen)


def name_version(entry) -> tuple:
    ident = entry[0]
    at = ident.find("@", 1)
    return ident[:at], ident[at + 1:]


def installed_dir(root: pathlib.Path, name: str, version: str) -> pathlib.Path:
    """The package's directory in Bun's isolated layout, node_modules/.bun/<name>@<version>[+<peers>]/node_modules/<name>."""
    store = root / "node_modules" / ".bun"
    stem = "%s@%s" % (name.replace("/", "+"), version)
    try:
        dirs = sorted(p for p in store.iterdir() if p.name == stem or p.name.startswith(stem + "+"))
    except OSError:
        dirs = []
    for d in dirs:
        pkg = d / "node_modules" / name
        try:
            meta = json.loads(read_text(pkg / "package.json"))
        except (NoVerdict, ValueError):
            continue
        if meta.get("version") == version:
            return pkg
    raise NoVerdict("%s@%s is not installed under %s as bun.lock says: run `bun install --frozen-lockfile` first"
                    % (name, version, store))


def declared_licence(pkg: pathlib.Path) -> str:
    """The licence its package.json declares, its spacing collapsed; "" when it names none,
    which the licence check refuses by name."""
    meta = json.loads(read_text(pkg / "package.json"))
    lic = meta.get("license")
    if isinstance(lic, dict):
        lic = lic.get("type")
    if not lic and isinstance(meta.get("licenses"), list):
        lic = " OR ".join(str(x.get("type", x) if isinstance(x, dict) else x) for x in meta["licenses"])
    return " ".join(str(lic).split()) if lic else ""


def bundled_keys(doc: dict, workspace: str, tools: dict) -> dict:
    """{bun.lock key: BundledTool} for each bundled tool, at the version the workspace's
    name for it resolves to; one the lock does not hold is no verdict, named."""
    ws = doc["workspaces"].get(workspace)
    if not isinstance(ws, dict):
        raise NoVerdict("bun.lock has no workspace %s" % workspace)
    out = {}
    for name in sorted(tools):
        key = resolve(doc["packages"], [ws.get("name", workspace)], name)
        if key is None:
            raise NoVerdict("bun.lock holds no %s, a bundled tool (BUNDLED_TOOLS in scripts/release/gen_notices.py)"
                            % name)
        out[key] = tools[name]
    return out


def bundled_notice(pkg: pathlib.Path, name: str, version: str, tool: BundledTool) -> str:
    """A bundled tool's own notice: the lines of its licence file before `ends_before`,
    which must be in it once. The file gone or its form changed is no verdict, never a
    substituted text, which would drop the tool's own copyright line."""
    where = "%s %s, a bundled tool, has no %s whose own notice ends before \"%s\"" % (
        name, version, tool.licence_file, tool.ends_before)
    if tool.licence_file not in {p.name for p in listing(pkg)}:
        raise NoVerdict("%s: it ships no such file; read its licence files again and update BUNDLED_TOOLS" % where)
    lines = normalize(read_text(pkg / tool.licence_file)).split("\n")
    if lines.count(tool.ends_before) != 1 or not "".join(lines[:lines.index(tool.ends_before)]).strip():
        raise NoVerdict("%s: that line is not in it once after a notice; read the file again and update "
                        "BUNDLED_TOOLS" % where)
    return "\n".join(lines[:lines.index(tool.ends_before)])


def js_half(root: pathlib.Path, doc: dict, keys: list, allowed, bundled=None) -> Half:
    """The packages the keys reach and the bundled tools (bundled_keys) they do not, with
    their licence texts. A package or tool whose licence the allowed licences do not
    satisfy is refused before any text is read, so the refusal names it; a package that
    ships no licence file is given the standard text of each id it ships under
    (spdx_allowed_ids); a bundled tool its own notice (bundled_notice)."""
    tools = {key: tool for key, tool in (bundled or {}).items() if key not in keys}
    installed = []
    for key in list(keys) + sorted(tools):
        name, version = name_version(doc["packages"][key])
        pkg = installed_dir(root, name, version)
        installed.append((name, version, declared_licence(pkg), pkg, tools.get(key)))
    refused = licence_refusals([p[:3] for p in installed], allowed)
    if refused:
        raise NoVerdict(refusal(refused))
    half = Half()
    for name, version, licence, pkg, tool in installed:
        half.packages.append((name, version, licence))
        if tool is not None:
            half.bundled[(name, version)] = tool
            half.add(bundled_notice(pkg, name, version, tool),
                     "%s as shipped, the lines before \"%s\"" % (tool.licence_file, tool.ends_before), (name, version))
            continue
        shipped = licence_files(pkg)
        for fname, text in shipped:
            half.add(text, "%s as shipped" % fname, (name, version))
        if not shipped:
            for ident in spdx_allowed_ids(licence, allowed):
                half.standard(root, ident, (name, version))
    half.packages = sorted(set(half.packages), key=package_key)
    return half


# ------------------------------------------------------------------ JavaScript: licences
def js_allowed(deny_text: str) -> list:
    """The licences a JavaScript package may ship under: deny.toml's allow list and
    JS_ALSO_ALLOWED."""
    return deny_allow(deny_text) + sorted(JS_ALSO_ALLOWED)


def spdx_parse(expression: str):
    """An SPDX licence expression (SPDX 2.3, annex D) as a tree: ("id", id), ("with", id,
    exception), ("and", [tree, ...]) or ("or", [tree, ...]). ValueError when it is none."""
    tokens = re.findall(r"[()]|[^\s()]+", expression)
    pos = 0

    def peek():
        return tokens[pos] if pos < len(tokens) else None

    def take():
        nonlocal pos
        if pos == len(tokens):
            raise ValueError("it ends early")
        pos += 1
        return tokens[pos - 1]

    def either():
        terms = [both()]
        while peek() == "OR":
            take()
            terms.append(both())
        return terms[0] if len(terms) == 1 else ("or", terms)

    def both():
        terms = [term()]
        while peek() == "AND":
            take()
            terms.append(term())
        return terms[0] if len(terms) == 1 else ("and", terms)

    def term():
        token = take()
        if token == "(":
            inner = either()
            if peek() != ")":
                raise ValueError("a ( is not closed")
            take()
            return inner
        if token in SPDX_OPERATORS or not SPDX_ID.fullmatch(token):
            raise ValueError("%r where a licence id or ( was due" % token)
        if peek() != "WITH":
            return ("id", token)
        take()
        exception = take()
        if exception in SPDX_OPERATORS or not SPDX_EXCEPTION.fullmatch(exception):
            raise ValueError("%r where an exception id was due" % exception)
        return ("with", token, exception)

    try:
        tree = either()
    except RecursionError:
        raise ValueError("it nests too deep") from None
    if pos != len(tokens):
        raise ValueError("%r after a whole expression" % tokens[pos])
    return tree


def spdx_satisfied(expression: str, allowed) -> bool:
    """Whether the allowed licences satisfy an SPDX expression: an id, or `id WITH
    exception`, when it is listed as written (ids in any case); A OR B when either is;
    A AND B when both are. ValueError when the expression is none."""
    return bool(spdx_allowed_ids(expression, allowed))


def spdx_allowed_ids(expression: str, allowed) -> list:
    """The licence and exception ids a package ships under, by the branches of its SPDX
    expression the allowed licences satisfy (spdx_satisfied's reading): an id itself;
    `X WITH Y`, X and Y; A AND B, both sides' ids; A OR B, the ids of each side that holds.
    Spelled as the list spells them and sorted; none when the expression is not satisfied.
    ValueError when it is no expression."""
    listed = {" ".join(a.split()).casefold(): " ".join(a.split()) for a in allowed}

    def ids(node):
        if node[0] in ("id", "with"):
            written = node[1] if node[0] == "id" else "%s WITH %s" % node[1:]
            spelled = listed.get(written.casefold())
            return None if spelled is None else {spelled.split()[0], spelled.split()[-1]}
        held = [ids(n) for n in node[1]]
        if node[0] == "and" and None in held:
            return None
        held = [h for h in held if h is not None]
        return set().union(*held) if held else None
    return sorted(ids(spdx_parse(expression)) or ())


def licence_refusals(packages, allowed) -> list:
    """One line for each (name, version, licence) the allowed licences do not satisfy."""
    out = []
    for name, version, licence in packages:
        if not licence.strip():
            out.append("%s %s: names no licence" % (name, version))
            continue
        try:
            if spdx_satisfied(licence, allowed):
                continue
            why = "not allowed"
        except ValueError as e:
            why = "not an SPDX licence expression: %s" % e
        out.append("%s %s: %s (%s)" % (name, version, licence, why))
    return out


def the_list() -> str:
    also = " and ".join(sorted(JS_ALSO_ALLOWED))
    return "deny.toml's [licenses] allow list" + (", with %s for JavaScript alone" % also if also else "")


def refusal(refused: list) -> str:
    return "\n".join(
        ["%d JavaScript package(s) refused, the licence not admitted by %s:" % (len(refused), the_list())]
        + ["  " + line for line in refused]
        + ["Allow a licence by name with its reason (deny.toml's list for both halves, or JS_ALSO_ALLOWED in"
           " scripts/release/gen_notices.py for JavaScript alone), or keep the package out."])


def js_section(text: str) -> list:
    """The (name, version, licence) lines of a notices file's JavaScript section, as render()
    writes them. NoVerdict unless the section is there once, each line reads, and their
    number is the header's count."""
    lines = text.split("\n")
    starts = [i for i in range(1, len(lines) - 2)
              if lines[i] == JS_TITLE and lines[i - 1] == RULE and lines[i + 1] == RULE and not lines[i + 2]]
    if len(starts) != 1:
        raise NoVerdict("%s has no single \"%s\" section" % (NOTICES, JS_TITLE))
    packages = []
    for line in itertools.takewhile(bool, lines[starts[0] + 3:]):
        m = PACKAGE_LINE.match(line)
        if not m:
            raise NoVerdict("%s: %r, in its %s section, is no `name version: licence` line" % (NOTICES, line, JS_TITLE))
        packages.append((m.group(1), m.group(2), (m.group(3) or "").strip()))
    counts = JS_COUNT.findall(header(text))
    if counts != [str(len(packages))]:
        raise NoVerdict("%s's %s section lists %d, but its header counts %s"
                        % (NOTICES, JS_TITLE, len(packages), " and ".join(counts) or "none"))
    return packages


def check_licences(root: pathlib.Path) -> int:
    """The committed file's JavaScript licences against js_allowed(): 0 all allowed, 1 not."""
    path = root / NOTICES
    if not path.is_file():
        raise NoVerdict("no %s at %s to check; generate it: %s" % (NOTICES, path, REGENERATE))
    packages = js_section(read_text(path))
    refused = licence_refusals(packages, js_allowed(read_text(root / "deny.toml")))
    if refused:
        print(refusal(refused))
        return 1
    print("%s: the %d JavaScript packages' licences are allowed by %s" % (NOTICES, len(packages), the_list()))
    return 0


# ------------------------------------------------------------------ the file
def version_key(version: str) -> tuple:
    return tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in re.split(r"[.+-]", version))


def package_key(package: tuple) -> tuple:
    return (package[0], version_key(package[1]), package[1])


def render_texts(title: str, half: Half) -> list:
    out = [RULE, "Licence texts: %s" % title, RULE, ""]
    def order(item):
        text, group = item
        return ("; ".join(sorted(group.labels)), sorted(map(package_key, group.used_by | group.substituted)), text)
    for text, group in sorted(half.groups.items(), key=order):
        out += [THIN, "; ".join(sorted(group.labels))]
        for name, version in sorted(group.used_by | group.substituted, key=lambda p: package_key(p)):
            tool = half.bundled.get((name, version))
            mark = (" (ships no licence file)" if (name, version) in group.substituted
                    else " (a build tool: the app's bundle carries %s)" % tool.carries if tool else "")
            out.append("  %s %s%s" % (name, version, mark))
        out += [THIN, "", text.rstrip("\n"), ""]
    for ident, packages in sorted(half.missing.items()):
        out += [THIN, "%s: no licence text available" % ident]
        out += ["  %s %s (ships no licence file; no standard text available)" % p
                for p in sorted(packages, key=package_key)]
        out += [THIN, ""]
    return out


def render(hashes: list, rust: Half, js: Half) -> str:
    out = [
        NOTICES,
        "",
        "The OpenAlpaca release carries the third-party software listed here: the",
        "Rust crates its daemon (openalpacad), CLI (openalpaca) and app",
        "(openalpaca_gui) link, the JavaScript packages its app's frontend ships,",
        "and the build tools whose runtime code that frontend carries. Each",
        "licence text is given once, after the packages it covers.",
        "",
        "Generated by scripts/release/gen_notices.py; do not edit by hand.",
        "scripts/release/package-macos.sh refuses to package while a hash below",
        "is not the tree's lockfile's.",
        "",
    ] + list(hashes) + [
        "Rust crates: cargo-about %s, %s" % (CARGO_ABOUT_VERSION, ABOUT_TOML),
        "JavaScript packages: %s's dependencies, closed over bun.lock" % GUI_WORKSPACE,
        "JavaScript bundled tools: %s" % (", ".join(sorted(name for name, _ in js.bundled)) or "none"),
        "Substituted texts: spdx/license-list-data %s, %s" % (SPDX_TAG, SPDX_DIR),
        "",
        "Rust crates: %d" % len(rust.packages),
        "Rust licence texts: %d" % len(rust.groups),
        "Rust crates given a substituted text: %d" % rust.substituted_count(),
        "Rust crates missing a licence text: %d" % rust.missing_count(),
        "%s: %d" % (JS_TITLE, len(js.packages)),
        "JavaScript licence texts: %d" % len(js.groups),
        "JavaScript packages given a substituted text: %d" % js.substituted_count(),
        "JavaScript packages missing a licence text: %d" % js.missing_count(),
        "",
    ]
    for title, half in (("Rust crates", rust), (JS_TITLE, js)):
        out += [RULE, title, RULE, ""]
        out += ["%s %s: %s" % p for p in half.packages] + [""]
    out += render_texts("Rust crates", rust) + render_texts("JavaScript packages", js)
    while out and not out[-1]:
        out.pop()
    return "\n".join(out) + "\n"


def generate(root: pathlib.Path, exe) -> str:
    check_accepted(read_text(root / ABOUT_TOML), read_text(root / "deny.toml"))
    hashes = hash_lines(root)
    rust = rust_half(root, run_cargo_about(root, find_cargo_about(exe)), deny_allow(read_text(root / "deny.toml")))
    doc = load_bun_lock(root / "bun.lock")
    js = js_half(root, doc, bun_closure(doc, GUI_WORKSPACE), js_allowed(read_text(root / "deny.toml")),
                 bundled_keys(doc, GUI_WORKSPACE, BUNDLED_TOOLS))
    if hash_lines(root) != hashes:
        raise NoVerdict("a lockfile changed while the notices were generated; run it again")
    return render(hashes, rust, js)


def main(argv: list) -> int:
    check, hashes, licences, root, exe, rest = False, False, False, ROOT, None, list(argv[1:])
    while rest:
        a = rest.pop(0)
        if a == "--check":
            check = True
        elif a == "--check-hashes":
            hashes = True
        elif a == "--check-licences":
            licences = True
        elif a in ("--root", "--cargo-about") and rest:
            v = rest.pop(0)
            if a == "--root":
                root = pathlib.Path(v).resolve()
            else:
                exe = v
        else:
            sys.stderr.write(__doc__)
            return 2
    if check + hashes + licences > 1:
        sys.stderr.write(__doc__)
        return 2
    path = root / NOTICES
    try:
        if licences:
            return check_licences(root)
        if check or hashes:
            if not path.is_file():
                print("%s is stale: there is none at %s; generate it: %s" % (NOTICES, path, REGENERATE))
                return 1
            current = read_text(path)
            stale = stale_locks(root, current)
            if not stale and hashes:
                print("%s records the hashes of the tree's %s and %s" % ((NOTICES,) + LOCKS))
                return 0
            if not stale:
                if generate(root, exe) == current:
                    print("%s is current" % NOTICES)
                    return 0
                stale = ["%s is stale: its hashes are the tree's lockfiles', but its text is not what the generator "
                         "writes now (a different node_modules, about.toml or vendored text?)" % NOTICES]
            for line in stale:
                print(line)
            print("Regenerate it (after bun install --frozen-lockfile): %s" % REGENERATE)
            return 1
        text = generate(root, exe)
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
    except NoVerdict as e:
        sys.stderr.write("gen_notices: %s\n" % e)
        return 2
    print("wrote %s (%d bytes)" % (path, len(text.encode("utf-8"))))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
