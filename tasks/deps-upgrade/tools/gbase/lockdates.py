#!/usr/bin/env python3
"""Registry dates for the dependency wave: an annotation of a lock report, and DU-23's gate.

Usage:
  lockdates.py < lock-differences.txt
  lockdates.py --gate --max-date <UTC> [--waivers FILE] [--cache FILE | --date-source FILE]
               PARENT.lock NEW.lock
  lockdates.py --pins crates|npm --max-date <UTC> [--waivers FILE] [--cache FILE | --date-source FILE]
               NAME@VERSION...

The annotation (no option; gbase.sh runs it only under --dates). For every
unexpected `name: old -> new` line of lockdiff.py's output, each version on the right
that is not on the left, and for every unexpected `+ name ver` line, prints

  name version YYYY-MM-DD   https://crates.io/crates/name/version

The date is the crates.io `created_at` of that version (UTC). A line lockdiff.py
labels (gbase.sh passes one label, `[DU-25(v4)]`: rmcp's 3.5.0 -> 3.5.1) is an
expected difference and is not looked up, and neither is the informational
`== base-vs-measured-head` block. A lookup that fails prints
`(date unavailable: <reason>)` on its line; the exit status is 0 regardless — this
mode annotates a report and decides nothing.

The gate (--gate; plan review 2026-10-17, MF-2). DU-23's rule: a release is eligible
7 days after its crates.io `created_at`, so at a cut instant C a version is eligible
when it was published at or before C - 7 days — the --max-date, an instant with its
zone (`2026-10-10T20:08Z` or `…:00Z`). For every (name, version) NEW.lock holds that
PARENT.lock does not — what a commit adds or moves — it reads crates.io and sorts it:
eligible (published at or before the max date), waived (listed in --waivers: a
recorded DU-23 (c) waiver), or too young. For a too-young entry it names the version to
step back to: the newest release of the same name that is semver-compatible with the
locked one (caret: the same major, or the same 0.minor, or the same 0.0.patch), older,
not yanked, not a pre-release unless the locked version is one, and published at or
before the max date — or the parent's own version of that name in that range, whatever
its age, when that is newer (keeping what the parent held is no move). It then prints
the `cargo update -p name@locked --precise target` sequence **dependents first** (an
entry is printed before every too-young entry it depends on, directly or not, by NEW.lock's
dependency lists — a dependent's requirement may bind its dependency, so stepping the
dependency first can fail, e.g. tauri-utils last), each spelled `name@locked`, plus one
machine line per entry: `step-back: name@locked target`, or `no-step-back: name@locked`
when nothing eligible exists. Run the commands one at a time and gate again after each:
the steps cascade (gbase.sh --apply-step-backs does exactly that). An entry from no
registry (a workspace member, a path) is not dated; an entry from another source (git,
another registry) cannot be, and is a failed lookup.

The pins check (--pins). Each NAME@VERSION (a scoped npm name keeps its leading `@`)
is sorted the same way, against crates.io or the npm registry's `time`; an npm pin's
exact-version optionalDependencies at that version (the @tauri-apps/cli platform
binaries, which bun.lock records and the minute table dated the cli by) are checked as
pins too.

Waivers (--waivers FILE): one `name@version  reason…` a line, `#` comments allowed.
A waiver covers exactly that name at that version, never a name, and a line with no
reason is refused (DU-23 (c): each waiver is recorded with its reason).

Registry reads: one request a second, a 20 s timeout each, the generic User-Agent
`openalpaca-deps-check` (never a name or an address); crates.io's
`/api/v1/crates/<name>` (every version with its created_at, one request per crate) and
`registry.npmjs.org/<name>`. --cache FILE keeps what was read, in the same JSON shape as
a date source, read at the start and written back at the end, so a gate run again after
each step-back reads each crate once per run; it is the record of the registry facts the
gate decided on. --date-source FILE is the same shape, read-only, and no network at all:
a name it does not hold is a failed lookup (gbase.sh's self-test). Shape:
  {"crates": {"<name>": [{"num": "1.2.3", "created_at": "<UTC>", "yanked": false}, …]},
   "npm": {"<name>": {"time": {"<version>": "<UTC>", …},
                      "optional": {"<version>": {"<dep>": "<spec>", …}}}}}

Exit status of --gate and --pins: 0 — every entry eligible or waived; 1 — at least one
entry too young (the gate printed its sequence) and only then; 2 — no verdict: a usage
error, a file that is not a Cargo.lock, a bad waiver line, or a failed lookup (in these
modes a lookup that fails fails the run, it never passes as "unavailable").
"""
import calendar
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

USER_AGENT = "openalpaca-deps-check"
API = "https://crates.io/api/v1/crates/%s/%s"
CRATE_API = "https://crates.io/api/v1/crates/%s"
NPM_API = "https://registry.npmjs.org/%s"
CRATES_IO = ("registry+https://github.com/rust-lang/crates.io-index", "sparse+https://index.crates.io/")


# ------------------------------------------------------------------ the annotation (unchanged)
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


def annotate():
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


# ------------------------------------------------------------------ shared by --gate and --pins
class NoVerdict(Exception):
    """Anything that leaves the gate without a verdict: exit 2."""


TS = re.compile(r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(Z|[+-]\d{2}:?\d{2})$")


def epoch(ts, what="timestamp"):
    m = TS.match(ts or "")
    if not m:
        raise NoVerdict("not an instant with its zone (%s): %r" % (what, ts))
    y, mo, d, h, mi, s, zone = m.groups()
    t = calendar.timegm((int(y), int(mo), int(d), int(h), int(mi), int(s or 0), 0, 0, 0))
    if zone != "Z":
        sign = 1 if zone[0] == "+" else -1
        zone = zone[1:].replace(":", "")
        t -= sign * (int(zone[:2]) * 3600 + int(zone[2:]) * 60)
    return t


def iso(t):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))


SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$")


def semver(v):
    m = SEMVER.match(v)
    if not m:
        return None
    pre = m.group(4)
    pre_key = ()
    if pre is not None:
        pre_key = tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in pre.split("."))
    return (int(m.group(1)), int(m.group(2)), int(m.group(3))), pre, pre_key


def order_key(v):
    s = semver(v)
    if s is None:
        return ((-1, -1, -1), 0, ())
    core, pre, pre_key = s
    return (core, 0 if pre is not None else 1, pre_key)


def compatible(a, b):
    """Caret compatibility: cargo's default requirement admits both."""
    sa, sb = semver(a), semver(b)
    if sa is None or sb is None:
        return False
    (ma, na, pa), (mb, nb, pb) = sa[0], sb[0]
    if ma != mb:
        return False
    if ma > 0:
        return True
    if na != nb:
        return False
    return na > 0 or pa == pb


def read_waivers(path):
    waived = {}
    if not path:
        return waived
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.read().splitlines()
    except (OSError, UnicodeDecodeError) as e:
        raise NoVerdict("cannot read the waivers file %s: %s" % (path, e))
    for n, line in enumerate(lines, 1):
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split(None, 1)
        if len(parts) < 2 or parts[0].rfind("@") <= 0:
            raise NoVerdict("%s:%d: a waiver is `name@version  reason`, with its reason (DU-23 (c)): %r" % (path, n, line))
        name, _, version = parts[0].rpartition("@")
        waived[(name, version)] = parts[1].strip()
    return waived


def split_pin(pin):
    name, at, version = pin.rpartition("@")
    if not at or not name or not version:
        raise NoVerdict("a pin is NAME@VERSION, not %r" % pin)
    return name, version


class Registry:
    def __init__(self, cache=None, source=None):
        self.cache, self.offline = cache, source is not None
        self.data = {"crates": {}, "npm": {}}
        self.last = 0.0
        path = source or cache
        if path and os.path.exists(path):
            try:
                with open(path, encoding="utf-8") as fh:
                    loaded = json.load(fh)
                self.data["crates"].update(loaded.get("crates", {}))
                self.data["npm"].update(loaded.get("npm", {}))
            except (OSError, ValueError) as e:
                raise NoVerdict("cannot read the date source %s: %s" % (path, e))
        elif source:
            raise NoVerdict("no date source at %s" % source)

    def save(self):
        if self.cache and not self.offline:
            tmp = self.cache + ".tmp"
            with open(tmp, "w", encoding="utf-8") as fh:
                json.dump(self.data, fh, indent=1, sort_keys=True)
            os.replace(tmp, self.cache)

    def _get(self, url):
        wait = 1.0 - (time.time() - self.last)
        if wait > 0:
            time.sleep(wait)
        self.last = time.time()
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=20) as resp:
                return json.load(resp)
        except (urllib.error.URLError, OSError, ValueError) as e:
            raise NoVerdict("lookup failed: %s (%s)" % (url, getattr(e, "reason", None) or e.__class__.__name__))

    def crate(self, name):
        if name not in self.data["crates"]:
            if self.offline:
                raise NoVerdict("lookup failed: crate %s is not in the date source" % name)
            doc = self._get(CRATE_API % urllib.parse.quote(name, safe=""))
            try:
                self.data["crates"][name] = [
                    {"num": v["num"], "created_at": v["created_at"], "yanked": bool(v.get("yanked"))}
                    for v in doc["versions"]
                ]
            except (KeyError, TypeError) as e:
                raise NoVerdict("lookup failed: crates.io answered for %s without a version list (%s)" % (name, e))
        return self.data["crates"][name]

    def npm(self, name, version):
        entry = self.data["npm"].get(name)
        if self.offline:
            if entry is None or version not in entry.get("time", {}):
                raise NoVerdict("lookup failed: npm package %s@%s is not in the date source" % (name, version))
        elif entry is None or version not in entry.get("time", {}) or version not in entry.get("optional", {}):
            doc = self._get(NPM_API % urllib.parse.quote(name, safe="@"))
            try:
                fresh = {"time": dict(doc["time"]), "optional": dict((entry or {}).get("optional", {}))}
            except (KeyError, TypeError) as e:
                raise NoVerdict("lookup failed: the npm registry answered for %s without `time` (%s)" % (name, e))
            man = (doc.get("versions") or {}).get(version)
            if man is not None:
                fresh["optional"][version] = dict(man.get("optionalDependencies") or {})
            self.data["npm"][name] = entry = fresh
        if version not in entry["time"]:
            raise NoVerdict("lookup failed: the npm registry has no %s@%s" % (name, version))
        return entry["time"][version], entry.get("optional", {}).get(version, {})


def crate_published(reg, name, version):
    for v in reg.crate(name):
        if v["num"] == version:
            return v["created_at"]
    raise NoVerdict("lookup failed: crates.io has no %s %s" % (name, version))


# ------------------------------------------------------------------ Cargo.lock with its edges
def parse_lock(path):
    """[{name, version, source, deps}] — deps as written: `name`, `name ver` or `name ver (src)`."""
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.read().splitlines()
    except (OSError, UnicodeDecodeError) as e:
        raise NoVerdict("cannot read %s as UTF-8 text: %s" % (path, e))
    pkgs, cur, in_deps = [], None, False
    for line in lines:
        if line == "[[package]]":
            cur = {"name": None, "version": None, "source": None, "deps": []}
            pkgs.append(cur)
            in_deps = False
            continue
        if cur is None:
            continue
        if in_deps:
            if line.strip() == "]":
                in_deps = False
            else:
                m = re.match(r'^\s*"(.*)",?\s*$', line)
                if m:
                    cur["deps"].append(m.group(1))
            continue
        m = re.match(r'^(name|version|source) = "(.*)"$', line)
        if m:
            cur[m.group(1)] = m.group(2)
            continue
        m = re.match(r"^dependencies = \[(.*)$", line)
        if m:
            rest = m.group(1).strip()
            if rest.endswith("]"):
                cur["deps"] += re.findall(r'"([^"]*)"', rest)
            else:
                in_deps = True
    pkgs = [p for p in pkgs if p["name"] and p["version"]]
    if not pkgs:
        raise NoVerdict("%s holds no [[package]] with a name and a version: not a Cargo.lock" % path)
    return pkgs


def dependents_first(pkgs, young):
    """Order the young (name, version) keys so that each comes before every young key it reaches."""
    by_name = {}
    for p in pkgs:
        by_name.setdefault(p["name"], []).append(p["version"])
    edges = {}
    for p in pkgs:
        out = []
        for d in p["deps"]:
            parts = d.split()
            if len(parts) >= 2:
                out.append((parts[0], parts[1]))
            elif parts and len(by_name.get(parts[0], [])) == 1:
                out.append((parts[0], by_name[parts[0]][0]))
        edges[(p["name"], p["version"])] = out
    young = set(young)
    reach = {}
    for y in young:
        seen, stack = set(), list(edges.get(y, []))
        while stack:
            k = stack.pop()
            if k in seen:
                continue
            seen.add(k)
            stack.extend(edges.get(k, []))
        reach[y] = seen & young
    ordered, left = [], set(young)
    while left:
        ready = sorted(k for k in left if not any(k in reach[o] for o in left if o != k))
        if not ready:  # a cycle cannot occur among registry crates; never loop on one
            ready = sorted(left)
        ordered.append(ready[0])
        left.discard(ready[0])
    return ordered


# ------------------------------------------------------------------ --gate and --pins
def gate(args, reg, max_t, waived):
    parent, new = parse_lock(args[0]), parse_lock(args[1])
    held = {}
    for p in parent:
        held.setdefault(p["name"], set()).add(p["version"])
    rows, young, targets, local = [], [], {}, 0
    for p in sorted(new, key=lambda p: (p["name"], order_key(p["version"]))):
        name, version, source = p["name"], p["version"], p["source"]
        if version in held.get(name, set()):
            continue
        if source is None:
            local += 1
            continue
        if not source.startswith(CRATES_IO):
            raise NoVerdict("lookup failed: %s %s comes from %s, which this gate cannot date" % (name, version, source))
        published = crate_published(reg, name, version)
        t = epoch(published, "%s %s's created_at" % (name, version))
        if t <= max_t:
            rows.append("  eligible   %s %s  %s" % (name, version, published))
        elif (name, version) in waived:
            rows.append("  waived     %s %s  %s  (DU-23 (c): %s)" % (name, version, published, waived[(name, version)]))
        else:
            cands = [
                v["num"]
                for v in reg.crate(name)
                if not v.get("yanked")
                and compatible(v["num"], version)
                and order_key(v["num"]) < order_key(version)
                and (semver(v["num"]) is not None and (semver(v["num"])[1] is None or semver(version)[1] is not None))
                and epoch(v["created_at"], "%s %s's created_at" % (name, v["num"])) <= max_t
            ]
            cands += [v for v in held.get(name, set()) if compatible(v, version) and order_key(v) < order_key(version)]
            target = max(cands, key=order_key) if cands else None
            targets[(name, version)] = target
            young.append((name, version))
            if target:
                rows.append("  TOO YOUNG  %s %s  %s  -> step back to %s" % (name, version, published, target))
            else:
                rows.append("  TOO YOUNG  %s %s  %s  -> no eligible version: a DU-23 (c) waiver, or a later cut" % (name, version, published))
    counts = {"eligible": sum(r.startswith("  eligible") for r in rows), "waived": sum(r.startswith("  waived") for r in rows)}
    print("freshness gate: %s -> %s; max date %s (a release published after it is younger than 7 days at the cut)" % (args[0], args[1], iso(max_t)))
    print("  added or moved: %d  eligible: %d  waived: %d  too young: %d  (not from a registry, not dated: %d)"
          % (len(rows), counts["eligible"], counts["waived"], len(young), local))
    for r in rows:
        print(r)
    if not young:
        return 0
    seq = dependents_first(new, young)
    print("the --precise sequence, dependents first — run one at a time and gate again after each (the steps cascade):")
    for k in seq:
        if targets[k]:
            print("  cargo update -p %s@%s --precise %s" % (k[0], k[1], targets[k]))
        else:
            print("  (no step-back for %s@%s)" % k)
    for k in seq:
        print(("step-back: %s@%s %s" % (k[0], k[1], targets[k])) if targets[k] else ("no-step-back: %s@%s" % k))
    return 1


def pins(kind, items, reg, max_t, waived):
    rows, young = [], 0
    todo = [split_pin(p) for p in items]
    seen = set()
    while todo:
        name, version = todo.pop(0)
        if (name, version) in seen:
            continue
        seen.add((name, version))
        if kind == "crates":
            published = crate_published(reg, name, version)
        else:
            published, optional = reg.npm(name, version)
            todo += [(d, spec) for d, spec in sorted(optional.items()) if SEMVER.match(spec or "")]
        t = epoch(published, "%s %s's publication time" % (name, version))
        if t <= max_t:
            rows.append("  eligible   %s %s  %s" % (name, version, published))
        elif (name, version) in waived:
            rows.append("  waived     %s %s  %s  (DU-23 (c): %s)" % (name, version, published, waived[(name, version)]))
        else:
            young += 1
            rows.append("  TOO YOUNG  %s %s  %s" % (name, version, published))
    print("pins (%s): max date %s; checked: %d  too young: %d" % (kind, iso(max_t), len(rows), young))
    for r in rows:
        print(r)
    return 1 if young else 0


def main(argv):
    if len(argv) == 1:
        return annotate()
    mode, kind, max_date, waivers, cache, source, rest = None, None, None, None, None, None, []
    i = 1
    while i < len(argv):
        a = argv[i]
        if a in ("--max-date", "--waivers", "--cache", "--date-source", "--pins"):
            if i + 1 >= len(argv):
                raise NoVerdict("%s needs a value\n%s" % (a, __doc__))
            v = argv[i + 1]
            i += 2
            if a == "--max-date":
                max_date = v
            elif a == "--waivers":
                waivers = v
            elif a == "--cache":
                cache = v
            elif a == "--date-source":
                source = v
            else:
                mode, kind = "pins", v
            continue
        if a == "--gate":
            mode = "gate"
        elif a in ("-h", "--help"):
            print(__doc__)
            return 0
        elif a.startswith("--"):
            raise NoVerdict("unknown option %s\n%s" % (a, __doc__))
        else:
            rest.append(a)
        i += 1
    if mode is None:
        raise NoVerdict("give --gate or --pins (or no argument for the annotation)\n%s" % __doc__)
    if not max_date or not max_date.endswith("Z"):
        raise NoVerdict("--max-date takes a UTC instant ending in Z, not %r" % max_date)
    if cache and source:
        raise NoVerdict("--cache and --date-source exclude each other")
    max_t = epoch(max_date, "--max-date")
    waived = read_waivers(waivers)
    reg = Registry(cache=cache, source=source)
    try:
        if mode == "gate":
            if len(rest) != 2:
                raise NoVerdict("--gate needs PARENT.lock NEW.lock\n%s" % __doc__)
            return gate(rest, reg, max_t, waived)
        if kind not in ("crates", "npm") or not rest:
            raise NoVerdict("--pins takes crates or npm, then NAME@VERSION...\n%s" % __doc__)
        return pins(kind, rest, reg, max_t, waived)
    finally:
        reg.save()


if __name__ == "__main__":
    try:
        status = main(sys.argv)
    except NoVerdict as e:
        sys.stdout.flush()
        sys.stderr.write("lockdates: no verdict: %s\n" % e)
        status = 2
    except Exception as e:  # an unforeseen error is no verdict either: never exit 1, which means "too young"
        import traceback

        traceback.print_exc()
        sys.stderr.write("lockdates: no verdict: stopped by an unforeseen error (%s: %s)\n" % (type(e).__name__, e))
        status = 2
    sys.exit(status)
