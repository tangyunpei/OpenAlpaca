#!/usr/bin/env python3.11
"""The workspace's crate-edge check (B-25): the manifests' internal edges against a table.

Usage:
  python3.11 -I scripts/crate_edges.py [--metadata FILE] [ROOT]

ROOT is the repository root (default: this file's parent's parent). The check runs
`cargo metadata --locked --format-version 1 --no-deps` in ROOT (or reads that document
from FILE, which its tests do), derives every internal edge — a workspace member naming
another under [dependencies], [build-dependencies] or [dev-dependencies] — and compares
them with the checked-in table, ROOT/scripts/crate_edges.toml. Exit 0 when they agree; 1
with one line per difference, each naming the member, the crate and the kind, and whether
the manifests add the edge or dropped it (a member added or dropped is named the same
way); 2 on a usage error, a table or metadata document it cannot read, or a cargo that
fails. It builds nothing and needs no network (`--no-deps` resolves nothing). There is no
remote CI (the owner, 2026-11-06): it is a local gate, run beside G-check.

An edge changes by changing the table in the same commit. The table lists every member,
a leaf too, under its package name, with the three kinds as arrays of package names; the
kinds are kept apart, so an edge moved from one kind to another is one added and one
removed. An internal edge is a dependency that cargo reads from a path and whose package
is a member: a registry crate sharing a member's name is not one. A renamed dependency
(`foo = { package = "openalpaca_x", … }`) is its package's edge, and a target-specific one
(`[target.'cfg(…)'.dependencies]`) counts as its kind's edge, the target not recorded.
CLAUDE.md's Dependency Graph block is not generated from the table; its normal edges are
the table's `dependencies`, kept in agreement by hand.
"""

import json
import pathlib
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
TABLE = pathlib.Path("scripts") / "crate_edges.toml"
CARGO = ["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"]

# cargo metadata's `kind` (null for a normal dependency) -> the manifest table's name.
KINDS = {None: "dependencies", "build": "build-dependencies", "dev": "dev-dependencies"}


class Unreadable(Exception):
    pass


Edges = dict[str, dict[str, set[str]]]


def load_table(path: pathlib.Path) -> Edges:
    try:
        doc = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as e:
        raise Unreadable("%s: %s" % (path, e)) from None
    out: Edges = {}
    for member, kinds in doc.items():
        if not isinstance(kinds, dict) or sorted(kinds) != sorted(KINDS.values()):
            raise Unreadable("%s: [%s] must hold exactly %s" % (path, member, ", ".join(sorted(KINDS.values()))))
        out[member] = {}
        for kind, deps in kinds.items():
            if not isinstance(deps, list) or not all(isinstance(d, str) for d in deps):
                raise Unreadable("%s: [%s] %s is not an array of package names" % (path, member, kind))
            if len(set(deps)) != len(deps):
                raise Unreadable("%s: [%s] %s names a crate twice" % (path, member, kind))
            out[member][kind] = set(deps)
    return out


def derive(metadata: dict) -> Edges:
    try:
        ids = set(metadata["workspace_members"])
        members = [p for p in metadata["packages"] if p["id"] in ids]
        names = {p["name"] for p in members}
        out: Edges = {name: {kind: set() for kind in KINDS.values()} for name in names}
        for p in members:
            for dep in p["dependencies"]:
                if "path" in dep and dep["name"] in names:
                    out[p["name"]][KINDS[dep["kind"]]].add(dep["name"])
    except (KeyError, TypeError) as e:
        raise Unreadable("the metadata document: no %s where cargo metadata puts one" % e) from None
    return out


def read_metadata(root: pathlib.Path, file: pathlib.Path | None) -> dict:
    if file is not None:
        try:
            text = file.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as e:
            raise Unreadable("%s: %s" % (file, e)) from None
        where = str(file)
    else:
        try:
            proc = subprocess.run(CARGO, cwd=root, capture_output=True, text=True, check=False)
        except OSError as e:
            raise Unreadable("`%s`: %s" % (" ".join(CARGO), e)) from None
        if proc.returncode != 0:
            raise Unreadable("`%s` exited %d: %s" % (" ".join(CARGO), proc.returncode, proc.stderr.strip()))
        text, where = proc.stdout, "`%s`" % " ".join(CARGO)
    try:
        doc = json.loads(text)
    except json.JSONDecodeError as e:
        raise Unreadable("%s: %s" % (where, e)) from None
    if not isinstance(doc, dict):
        raise Unreadable("%s: not a JSON object" % where)
    return doc


def compare(table: Edges, found: Edges) -> list[str]:
    out = []
    for member in sorted(found.keys() - table.keys()):
        out.append("crate_edges: member added: %s: a workspace member %s does not list" % (member, TABLE))
    for member in sorted(table.keys() - found.keys()):
        out.append("crate_edges: member removed: %s: %s lists it, the workspace has no such member" % (member, TABLE))
    for member in sorted(found.keys() & table.keys()):
        for kind in sorted(KINDS.values()):
            for dep in sorted(found[member][kind] - table[member][kind]):
                out.append("crate_edges: edge added: %s -> %s [%s]: the manifests name it, %s does not"
                           % (member, dep, kind, TABLE))
            for dep in sorted(table[member][kind] - found[member][kind]):
                out.append("crate_edges: edge removed: %s -> %s [%s]: %s names it, the manifests do not"
                           % (member, dep, kind, TABLE))
    return out


def main(argv: list[str]) -> int:
    args, file = argv[1:], None
    if args[:1] == ["--metadata"] and len(args) >= 2:
        file, args = pathlib.Path(args[1]), args[2:]
    if len(args) > 1 or (args and args[0].startswith("-")):
        sys.stderr.write(__doc__)
        return 2
    root = pathlib.Path(args[0]).resolve() if args else ROOT
    if not root.is_dir():
        sys.stderr.write("crate_edges: %s is not a directory\n" % root)
        return 2
    try:
        table = load_table(root / TABLE)
        found = derive(read_metadata(root, file))
    except Unreadable as e:
        sys.stderr.write("crate_edges: cannot read %s\n" % e)
        return 2
    diffs = compare(table, found)
    for line in diffs:
        print(line)
    print("crate_edges: %d member(s), %d edge(s), %d difference(s)"
          % (len(found), sum(len(d) for kinds in found.values() for d in kinds.values()), len(diffs)))
    return 1 if diffs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
