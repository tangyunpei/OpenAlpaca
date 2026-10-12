#!/usr/bin/env python3
"""Look up on crates.io when the versions a lock comparison introduced were published.

Usage: lockdates.py < lock-differences.txt        (lockdiff.py's output, any number of runs)

For every unexpected `name: old -> new` line, each version on the right that is
not on the left, and for every unexpected `+ name ver` line, prints

  name version YYYY-MM-DD   https://crates.io/crates/name/version

The date is the crates.io `created_at` of that version (UTC), so a human can tell a
release dated after 2026-10-10 — expected under DU-23 (a) — from anything else.
One request a second, a 20 s timeout each, and the generic User-Agent
`openalpaca-deps-check` (never a name or an address). A lookup that fails prints
`(date unavailable: <reason>)` on its line; the exit status is 0 regardless — this
is an annotation of a report, not a gate. gbase.sh runs it only under --dates.

A line lockdiff.py labels (gbase.sh passes one label, `[DU-25(v4)]`: rmcp's 3.5.0 ->
3.5.1) is an expected difference and is not looked up, and neither is the informational
`== base-vs-measured-head` block, so a run asks crates.io about the unexpected
differences alone.
"""
import json
import re
import sys
import time
import urllib.error
import urllib.request

USER_AGENT = "openalpaca-deps-check"
API = "https://crates.io/api/v1/crates/%s/%s"


def wanted(lines):
    """Yield (name, version) for every version a difference line introduces."""
    seen = set()
    block = ""
    for line in lines:
        line = line.rstrip("\n")
        if line.startswith("== "):
            block = line[3:].split(":", 1)[0]
            continue
        if block == "base-vs-measured-head" or re.search(r"\s\[[^\]]*\]\s*$", line):
            continue  # information, or an expected difference
        m = re.match(r"^\s+([^\s:]+): (.*) -> (.*)$", line)
        if m:
            name = m.group(1)
            old = {v.strip() for v in m.group(2).split(",")}
            for v in m.group(3).split(","):
                v = v.strip()
                if v and v not in old and (name, v) not in seen:
                    seen.add((name, v))
                    yield name, v
            continue
        m = re.match(r"^\s+\+ (\S+) (.*)$", line)
        if m:
            name = m.group(1)
            for v in m.group(2).split(","):
                v = v.strip()
                if v and (name, v) not in seen:
                    seen.add((name, v))
                    yield name, v


def created_at(name, version):
    req = urllib.request.Request(API % (name, version), headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=20) as resp:
        data = json.load(resp)
    return data["version"]["created_at"][:10]


def main():
    first = True
    for name, version in wanted(sys.stdin):
        if not first:
            time.sleep(1.0)
        first = False
        url = "https://crates.io/crates/%s/%s" % (name, version)
        try:
            date = created_at(name, version)
        except (urllib.error.URLError, OSError, KeyError, ValueError) as e:
            date = "(date unavailable: %s)" % (getattr(e, "reason", None) or e.__class__.__name__)
        print("%s %s %s   %s" % (name, version, date, url))
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
