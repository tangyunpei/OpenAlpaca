#!/usr/bin/env python3
"""Compare two Cargo.lock files as sets of (name, version); say which differences are expected.

Usage:
  lockdiff.py [--expected LABEL=NAMES]... OLD.lock NEW.lock
  lockdiff.py --names OLD.lock NEW.lock

NAMES is a comma-separated list of package names, or the path of a file holding one
name per line (`#` comments allowed). A difference on a listed name is printed with
its LABEL and is not counted as unexpected; every other difference is.

Output: a summary line, then one line per difference —
  name: old -> new          a package whose version set changed
  + name ver                a package the new lock adds
  - name ver                a package the new lock drops
each followed by `[LABEL]` when it is expected. A package held at two versions is
listed with both.

Exit status: 0 when the locks are equal or every difference is expected; 1 when at
least one difference is unexpected; 2 on a usage error or an unreadable file.
`--names` prints only the names of the packages that differ (one per line, exit 0) —
how `hold-footprint.txt` was derived.
"""
import collections
import os
import re
import sys


def load(path):
    pkgs = collections.defaultdict(set)
    name = None
    try:
        fh = open(path, encoding="utf-8")
    except OSError as e:
        sys.exit("lockdiff: cannot read %s: %s" % (path, e))
    with fh:
        for line in fh:
            line = line.rstrip("\n")
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
    return pkgs


def read_names(spec):
    if os.path.exists(spec):
        names = set()
        with open(spec, encoding="utf-8") as fh:
            for line in fh:
                line = line.split("#", 1)[0].strip()
                if line:
                    names.add(line)
        return names
    return {n.strip() for n in spec.split(",") if n.strip()}


def main(argv):
    expected = []  # [(label, names)]
    names_only = False
    files = []
    i = 1
    while i < len(argv):
        a = argv[i]
        if a == "--expected":
            i += 1
            if i >= len(argv) or "=" not in argv[i]:
                sys.exit("lockdiff: --expected takes LABEL=NAMES")
            label, spec = argv[i].split("=", 1)
            expected.append((label, read_names(spec)))
        elif a == "--names":
            names_only = True
        elif a.startswith("-"):
            sys.exit("lockdiff: unknown option %s\n%s" % (a, __doc__))
        else:
            files.append(a)
        i += 1
    if len(files) != 2:
        sys.exit("lockdiff: need OLD.lock NEW.lock\n%s" % __doc__)

    old, new = load(files[0]), load(files[1])
    rows = []  # (kind, name, text)
    for n in sorted(set(old) | set(new)):
        o, w = old.get(n, set()), new.get(n, set())
        if o == w:
            continue
        if not o:
            rows.append(("added", n, "  + %s %s" % (n, ", ".join(sorted(w)))))
        elif not w:
            rows.append(("removed", n, "  - %s %s" % (n, ", ".join(sorted(o)))))
        else:
            rows.append(("moved", n, "  %s: %s -> %s" % (n, ", ".join(sorted(o)), ", ".join(sorted(w)))))

    if names_only:
        for _, n, _ in rows:
            print(n)
        return 0

    def label_of(name):
        for label, names in expected:
            if name in names:
                return label
        return None

    counts = collections.Counter(k for k, _, _ in rows)
    by_label = collections.Counter()
    unexpected = 0
    for _, n, _ in rows:
        lab = label_of(n)
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
    for _, n, text in rows:
        lab = label_of(n)
        print(text + ("  [%s]" % lab if lab else ""))
    return 1 if unexpected else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
