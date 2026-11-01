#!/usr/bin/env python3
"""The repository-policy guard (B-23): what the workflows and the gate scripts may not do.

Usage:
  python3 -I scripts/repo_policy.py [ROOT]

ROOT is the repository root (default: this file's parent's parent). Exit 0 when every
rule holds; 1 with one line per violation, `path:line: rule: what`; 2 on a usage error,
a file the guard cannot read, or a ROOT with no .github/workflows. CI's `tooling` job
runs it; so can anyone, offline.

The rules:
  locked     every `cargo build`, `test`, `clippy` and `check` a workflow runs, whatever
             global options come before the subcommand (`cargo -q build`), is
             `--locked` (or `--frozen`, which implies it).
  sha-pin    every `uses:` of a workflow names its action by a 40-hex commit SHA,
             followed by its `# vX.Y.Z` comment; a local `./` action has no ref.
  checkout   every `actions/checkout` step sets `persist-credentials: false`.
  toolchain  no workflow or gate script names a toolchain as a `cargo +1.9x.y` literal:
             the toolchain is rust-toolchain.toml's. The G-base driver's
             `cargo "+$TOOLCHAIN"` is a parameter, not a literal — the named exception
             the rule's form leaves out by itself.
  build-dir  every CARGO_TARGET_DIR a workflow or a gate script sets is paired with
             CARGO_BUILD_BUILD_DIR (M63, MF-5): on the same command line, on an adjacent
             line of the same script, or as a sibling key of the same `env:` mapping.
  zip8       while deny.toml or .cargo/audit.toml ignores RUSTSEC-2026-0367, -0368 or
             -0369 (zip < 9, calamine's zip 8.6.0), Cargo.lock holds a zip 8 — so the
             ignores are lifted when calamine moves to zip 9, rather than remembered.

The workflows are .github/workflows/*.yml and *.yaml. The gate scripts are the *.sh,
*.py, *.ps1, *.ts and *.mjs files under scripts/, tasks/deps-upgrade/tools/ and
apps/openalpaca-gui/scripts/. A comment is not a command: `#` lines (and `//`, `/*`,
`*` lines in TypeScript) are skipped.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]

GATE_DIRS = ("scripts", "tasks/deps-upgrade/tools", "apps/openalpaca-gui/scripts")
GATE_SUFFIXES = (".sh", ".py", ".ps1", ".ts", ".mjs")
ZIP_IDS = ("RUSTSEC-2026-0367", "RUSTSEC-2026-0368", "RUSTSEC-2026-0369")

# Named exceptions: (rule, path, a substring of the line) -> why.
EXCEPTIONS = {
    ("build-dir", "tasks/deps-upgrade/tools/gbase/gbase.sh", "selftest-13b-build-dir-control"):
        "the self-test's control case: it unsets CARGO_BUILD_BUILD_DIR to prove where an unpaired build's intermediates go",
}

# `cargo`, its `+toolchain`, its global options (`-q`, `-Zflag`, and `--color`, `--config`,
# `--explain`, `-C`, `-Z` with their value as the next word), then the subcommand, a whole
# word: so `cargo -q build` is read, and `cargo --config test.jobs=4 metadata` is not a test.
CARGO_SUB = re.compile(
    r"\bcargo(?:\s+\+\S+)?"
    r"(?:\s+(?:(?:--color|--config|--explain|-C|-Z)\s+(?!-)\S+|-\S+))*"
    r"\s+(build|test|clippy|check)(?![\w.=-])"
)
TOOLCHAIN_LITERAL = re.compile(r"""\bcargo\s+["']?\+\d+\.\d+(?:\.\d+)?\b""")
USES = re.compile(r"^(\s*)(?:-\s+)?uses:\s*([^\s#]+)\s*(?:#\s*(.*))?$")
SHA_REF = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:/[^@\s]+)?@[0-9a-f]{40}$")
VERSION_COMMENT = re.compile(r"^v\d+\.\d+\.\d+\b")
SHELL_TARGET = re.compile(r"\bCARGO_TARGET_DIR=")
SHELL_BUILD = re.compile(r"\bCARGO_BUILD_BUILD_DIR=")
YAML_TARGET = re.compile(r"^(\s*)CARGO_TARGET_DIR\s*:")


class Unreadable(Exception):
    pass


def read(path: pathlib.Path) -> list[str]:
    try:
        return path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError) as e:
        raise Unreadable("%s: %s" % (path, e))


def is_comment(line: str, suffix: str) -> bool:
    s = line.lstrip()
    if s.startswith("#"):
        return True
    return suffix in (".ts", ".mjs") and s.startswith(("//", "/*", "*"))


def strip_comment(line: str) -> str:
    """The line without a trailing `# …` comment that starts outside quotes."""
    quote = None
    for i, c in enumerate(line):
        if quote:
            if c == quote:
                quote = None
        elif c in "\"'":
            quote = c
        elif c == "#" and (i == 0 or line[i - 1] in " \t"):
            return line[:i]
    return line


def logical_lines(lines: list[str], suffix: str):
    """(first line number, text) with backslash continuations joined and comments dropped."""
    buf, start = [], None
    for n, line in enumerate(lines, 1):
        if is_comment(line, suffix) and not buf:
            continue
        text = strip_comment(line).rstrip()
        if start is None:
            start = n
        if text.endswith("\\"):
            buf.append(text[:-1])
            continue
        buf.append(text)
        yield start, " ".join(buf)
        buf, start = [], None
    if buf:
        yield start, " ".join(buf)


def excepted(rule: str, rel: str, line: str) -> bool:
    return any(r == rule and p == rel and s in line for (r, p, s) in EXCEPTIONS)


def workflows(root: pathlib.Path) -> list[pathlib.Path]:
    d = root / ".github" / "workflows"
    return sorted(p for p in d.glob("*") if p.suffix in (".yml", ".yaml")) if d.is_dir() else []


def gate_scripts(root: pathlib.Path) -> list[pathlib.Path]:
    out = []
    for d in GATE_DIRS:
        base = root / d
        if base.is_dir():
            out += [p for p in base.rglob("*") if p.is_file() and p.suffix in GATE_SUFFIXES]
    return sorted(out)


def check_locked(rel, lines, out):
    for n, text in logical_lines(lines, ".yml"):
        for segment in re.split(r"&&|\|\||;|\|", text):
            m = CARGO_SUB.search(segment)
            if m and not re.search(r"(?<![\w-])--(locked|frozen)\b", segment):
                out.append("%s:%d: locked: `cargo %s` without --locked" % (rel, n, m.group(1)))


def check_uses(rel, lines, out):
    for n, line in enumerate(lines, 1):
        m = USES.match(line)
        if not m or line.lstrip().startswith("#"):
            continue
        ref, comment = m.group(2).strip("'\""), (m.group(3) or "").strip()
        if ref.startswith("./"):
            continue
        if not SHA_REF.match(ref):
            out.append("%s:%d: sha-pin: `%s` is not pinned by a full commit SHA" % (rel, n, ref))
        elif not VERSION_COMMENT.match(comment):
            out.append("%s:%d: sha-pin: `%s` has no `# vX.Y.Z` comment" % (rel, n, ref))


def step_body(lines, i, indent):
    """The lines of the step whose `uses:` is lines[i] (0-based), up to the next item at its level."""
    body = []
    for line in lines[i + 1:]:
        if line.strip() and (len(line) - len(line.lstrip())) <= indent:
            break
        body.append(line)
    return body


def step_start(lines, i):
    """(index, indent of its `-`) of the list item — the step — that holds lines[i]."""
    indent = len(lines[i]) - len(lines[i].lstrip())
    for j in range(i, -1, -1):
        s = lines[j].lstrip()
        k = len(lines[j]) - len(s)
        if s.startswith("- ") and (j == i or k < indent):
            return j, k
    return i, indent


def check_checkout(rel, lines, out):
    for i, line in enumerate(lines):
        m = USES.match(line)
        if not m or not m.group(2).strip("'\"").startswith("actions/checkout@"):
            continue
        start, dash = step_start(lines, i)  # the whole step, whatever its keys' order
        body = [lines[start][dash + 1:]] + step_body(lines, start, dash)
        if not any(re.match(r"^\s*persist-credentials:\s*['\"]?false['\"]?\s*(#.*)?$", b) for b in body):
            out.append("%s:%d: checkout: actions/checkout keeps its credentials (no `persist-credentials: false`)" % (rel, i + 1))


def check_toolchain(rel, lines, suffix, out):
    for n, line in enumerate(lines, 1):
        if is_comment(line, suffix):
            continue
        m = TOOLCHAIN_LITERAL.search(strip_comment(line))
        if m and not excepted("toolchain", rel, line):
            out.append("%s:%d: toolchain: `%s` names a toolchain; the pinned one comes from rust-toolchain.toml" % (rel, n, m.group(0)))


def check_build_dir_shell(rel, lines, suffix, out):
    texts = [None if is_comment(l, suffix) else strip_comment(l) for l in lines]
    for i, t in enumerate(texts):
        if not t or not SHELL_TARGET.search(t) or excepted("build-dir", rel, lines[i]):
            continue
        # the command line it is on (continuations either side), then the adjacent lines
        lo, hi = i, i
        while lo > 0 and texts[lo - 1] and texts[lo - 1].rstrip().endswith("\\"):
            lo -= 1
        while hi + 1 < len(texts) and texts[hi] and texts[hi].rstrip().endswith("\\"):
            hi += 1
        near = [x for x in texts[max(0, lo - 1):hi + 2] if x]
        if not any(SHELL_BUILD.search(x) for x in near):
            out.append("%s:%d: build-dir: CARGO_TARGET_DIR set without CARGO_BUILD_BUILD_DIR beside it" % (rel, i + 1))


def check_build_dir_yaml(rel, lines, out):
    for i, line in enumerate(lines):
        m = YAML_TARGET.match(line)
        if not m:
            continue
        indent = len(m.group(1))
        siblings = []
        for step in (-1, 1):  # the contiguous mapping around it, at its indent
            j = i + step
            while 0 <= j < len(lines):
                l = lines[j]
                if not l.strip():
                    j += step
                    continue
                k = len(l) - len(l.lstrip())
                if k < indent:
                    break
                if k == indent:
                    siblings.append(l)
                j += step
        if not any(re.match(r"^\s*CARGO_BUILD_BUILD_DIR\s*:", s) for s in siblings):
            out.append("%s:%d: build-dir: CARGO_TARGET_DIR set without CARGO_BUILD_BUILD_DIR beside it" % (rel, i + 1))
    check_build_dir_shell(rel, lines, ".yml", out)


def ignored_zip_ids(path: pathlib.Path):
    """[(line, id)] of the zip ids a TOML file names outside comments."""
    if not path.is_file():
        return []
    hits = []
    for n, line in enumerate(read(path), 1):
        body = strip_comment(line)
        hits += [(n, i) for i in ZIP_IDS if i in body]
    return hits


def locked_zip_majors(lock: pathlib.Path) -> list[str]:
    text = "\n".join(read(lock)) if lock.is_file() else ""
    return re.findall(r'(?m)^\[\[package\]\]\nname = "zip"\nversion = "(\d+)\.', text)


def check_zip8(root, out):
    for rel in ("deny.toml", ".cargo/audit.toml"):
        hits = ignored_zip_ids(root / rel)
        if hits and "8" not in locked_zip_majors(root / "Cargo.lock"):
            n, ident = hits[0]
            out.append("%s:%d: zip8: %s is ignored but Cargo.lock holds no zip 8 — lift the zip ignores (calamine has moved)" % (rel, n, ident))


def check(root: pathlib.Path) -> list[str]:
    out: list[str] = []
    for path in workflows(root):
        rel, lines = path.relative_to(root).as_posix(), read(path)
        check_locked(rel, lines, out)
        check_uses(rel, lines, out)
        check_checkout(rel, lines, out)
        check_toolchain(rel, lines, ".yml", out)
        check_build_dir_yaml(rel, lines, out)
    for path in gate_scripts(root):
        rel, lines = path.relative_to(root).as_posix(), read(path)
        check_toolchain(rel, lines, path.suffix, out)
        check_build_dir_shell(rel, lines, path.suffix, out)
    check_zip8(root, out)
    return out


def main(argv: list[str]) -> int:
    if len(argv) > 2 or (len(argv) == 2 and argv[1].startswith("-")):
        sys.stderr.write(__doc__)
        return 2
    root = pathlib.Path(argv[1]).resolve() if len(argv) == 2 else ROOT
    if not (root / ".github" / "workflows").is_dir():
        sys.stderr.write("repo_policy: %s has no .github/workflows\n" % root)
        return 2
    try:
        found = check(root)
    except Unreadable as e:
        sys.stderr.write("repo_policy: cannot read %s\n" % e)
        return 2
    for line in found:
        print(line)
    print("repo_policy: %d workflow(s), %d gate script(s), %d violation(s)"
          % (len(workflows(root)), len(gate_scripts(root)), len(found)))
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
