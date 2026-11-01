#!/usr/bin/env python3
"""Compare two Cargo.lock files as sets of (name, version); say which differences are expected.

Usage:
  lockdiff.py [--expected LABEL=SPECS]... OLD.lock NEW.lock
  lockdiff.py --names OLD.lock NEW.lock

SPECS is a comma-separated list, or the path of a file holding one spec per line (`#`
comments allowed). A spec is either a package name — any difference on that name is
expected — or `name:OLD->NEW`, which expects exactly that move: the old lock holds
`name` at OLD alone and the new lock at NEW alone (gbase.sh's rmcp 3.5.0 -> 3.5.1,
DU-25 (v4); the same name moving anywhere else stays unexpected). A difference a spec
covers is printed with its LABEL and is not counted as unexpected; every other is.

Output: a summary line, then one line per difference —
  name: old -> new          a package whose version set changed
  + name ver                a package the new lock adds
  - name ver                a package the new lock drops
each followed by `[LABEL]` when it is expected. A package held at two versions is
listed with both.

Exit status: 0 when the locks are equal or every difference is expected; 1 when at
least one difference is unexpected — and only then; 2 on anything else: a usage error,
a file it cannot read, a file that is not a Cargo.lock (not UTF-8 text, or no
`[[package]]` with a name and a version), or any error the code did not foresee (its
traceback goes to stderr). gbase.sh's compare_lock reads 1 as "a difference was found",
so nothing but a finished comparison may exit 1; it also refuses any output whose first
line is not the summary, whatever the exit status.
`--names` prints only the names of the packages that differ (one per line, exit 0).
"""
import collections
import os
import re
import sys
import traceback


def fail(message):
    """Anything but a finished comparison: exit 2, as documented. (A bare sys.exit(str),
    or an uncaught exception, exits 1, which gbase.sh's compare_lock reads as "a
    difference was found".)"""
    sys.stderr.write(message + "\n")
    sys.exit(2)


def read_lines(path, what):
    """The file's lines, or exit 2 when it cannot be read as UTF-8 text."""
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read().splitlines()
    except OSError as e:
        fail("lockdiff: cannot read %s %s: %s" % (what, path, e))
    except UnicodeDecodeError as e:
        fail("lockdiff: %s %s is not UTF-8 text: %s" % (what, path, e))


def load(path):
    pkgs = collections.defaultdict(set)
    name = None
    for line in read_lines(path, "lock"):
        if line == "[[package]]":
            name = None
            continue
        m = re.match(r'^name = "(.*)"$', line)
        if m:
            name = m.group(1)
            continue
        m = re.match(r'^version = "(.*)"$', line)
        if m and name is not None:
            pkgs[name].add(m.group(1))
            name = None
    if not pkgs:
        fail("lockdiff: %s holds no [[package]] with a name and a version: not a Cargo.lock" % path)
    return pkgs


def read_specs(spec):
    """Return (names, moves): names expected to differ in any way, and {name: (old, new)}
    for the `name:OLD->NEW` specs, each side a one-version set."""
    if os.path.exists(spec):
        items = [line.split("#", 1)[0].strip() for line in read_lines(spec, "expectation file")]
    else:
        items = [n.strip() for n in spec.split(",")]
    names, moves = set(), {}
    for item in items:
        if not item:
            continue
        m = re.match(r"^([^\s:]+):([^\s>-][^\s>]*)->(\S+)$", item)
        if m:
            moves[m.group(1)] = (frozenset([m.group(2)]), frozenset([m.group(3)]))
        elif re.match(r"^[^\s:]+$", item):
            names.add(item)
        else:
            fail("lockdiff: bad expectation %r (want NAME or NAME:OLD->NEW)" % item)
    return names, moves


def main(argv):
    expected = []  # [(label, names, moves)]
    names_only = False
    files = []
    i = 1
    while i < len(argv):
        a = argv[i]
        if a == "--expected":
            i += 1
            if i >= len(argv) or "=" not in argv[i]:
                fail("lockdiff: --expected takes LABEL=SPECS")
            label, spec = argv[i].split("=", 1)
            expected.append((label,) + read_specs(spec))
        elif a == "--names":
            names_only = True
        elif a.startswith("-"):
            fail("lockdiff: unknown option %s\n%s" % (a, __doc__))
        else:
            files.append(a)
        i += 1
    if len(files) != 2:
        fail("lockdiff: need OLD.lock NEW.lock\n%s" % __doc__)

    old, new = load(files[0]), load(files[1])
    rows = []  # (kind, name, text, old set, new set)
    for n in sorted(set(old) | set(new)):
        o, w = old.get(n, set()), new.get(n, set())
        if o == w:
            continue
        if not o:
            rows.append(("added", n, "  + %s %s" % (n, ", ".join(sorted(w))), o, w))
        elif not w:
            rows.append(("removed", n, "  - %s %s" % (n, ", ".join(sorted(o))), o, w))
        else:
            rows.append(("moved", n, "  %s: %s -> %s" % (n, ", ".join(sorted(o)), ", ".join(sorted(w))), o, w))

    if names_only:
        for _, n, _, _, _ in rows:
            print(n)
        return 0

    def label_of(name, o, w):
        for label, names, moves in expected:
            if name in names:
                return label
            if name in moves and moves[name] == (frozenset(o), frozenset(w)):
                return label
        return None

    counts = collections.Counter(r[0] for r in rows)
    by_label = collections.Counter()
    unexpected = 0
    for _, n, _, o, w in rows:
        lab = label_of(n, o, w)
        if lab is None:
            unexpected += 1
        else:
            by_label[lab] += 1
    extra = "".join("  %s: %d" % (k, v) for k, v in sorted(by_label.items()))
    print(
        "moved: %d  added: %d  removed: %d  unexpected: %d%s  entries: %d -> %d"
        % (
            counts["moved"],
            counts["added"],
            counts["removed"],
            unexpected,
            extra,
            sum(len(v) for v in old.values()),
            sum(len(v) for v in new.values()),
        )
    )
    for _, n, text, o, w in rows:
        lab = label_of(n, o, w)
        print(text + ("  [%s]" % lab if lab else ""))
    return 1 if unexpected else 0


if __name__ == "__main__":
    try:
        status = main(sys.argv)
    except Exception as e:  # SystemExit is not an Exception: fail()'s exit 2 passes through
        traceback.print_exc()
        fail("lockdiff: stopped by an unforeseen error (%s: %s); no comparison was made" % (type(e).__name__, e))
    sys.exit(status)
