#!/usr/bin/env python3
"""Split the re-cut's source diff into the three per-step patches gbase.sh applies.

Usage: python3 -I split-recut-diff.py recut-source-only.diff patches/

The whole wave's source changes against 7295f91b (`recut-source-only.diff`, cut on
2026-10-10, plan-rev3 §5.1 item 6) carry hunks of three steps of the eight-step
sequence; the other five steps change manifests only, which gbase.sh edits by
anchored line replacement, so the four manifests' hunks are excluded here and
reproduced by those edits (gbase.sh --edits-only proves the two agree).

Assignment, by path (plan-rev3 §5.3 P10, P11, P19, P30):

  step 4 (secrets)   crates/openalpaca_llm/src/keys/key_encryption/{mod,tests}.rs
                     and the eleven `format!("{:x}", <digest>)` sites of P10
  step 6 (rmcp)      crates/openalpaca_mcp/**, the core tool layer (tools/mcp,
                     tools/config/annotations.rs, tools/registry/*, tools/builtins/mod.rs),
                     security/sandbox/tests.rs, tests/mcp_integration.rs, the daemon's
                     managers/mcp.rs and routes/tools/tests.rs
  step 8 (config)    crates/openalpaca_core/src/middleware/skill/*

One file carries hunks of two steps: apps/openalpacad/src/managers/mcp.rs holds a
P10 digest site (`content_hash`, the `{:x}` hunk) and a P19 rmcp hunk
(`Implementation::new`). It is split by hunk — the `{:x}` hunk to step 4, the rest to
step 6 — because sha2 0.11 does not compile the old digest formatting (step 4's
check would fail) and rmcp 0.16 has no `Implementation::new` (step 4's check would
fail the other way). Both hunks apply against the base tree in either order.

`index` lines are dropped: a split file's blob ids describe neither intermediate
state, and `git apply` (without --index/--3way) never reads them.

The script asserts: every file of the diff is a manifest or assigned to one step;
exactly eleven `{:x}` hunks go to step 4; no hunk is left behind.
"""
import os
import re
import sys

MANIFESTS = {
    "Cargo.toml",
    "rust-toolchain.toml",
    "crates/openalpaca_core/Cargo.toml",
    "crates/openalpaca_mcp/Cargo.toml",
}

STEP_OF_PATH = [
    # (step, predicate)
    (4, lambda p: p.startswith("crates/openalpaca_llm/src/keys/key_encryption/")),
    (4, lambda p: p == "crates/openalpaca_storage/src/content_io.rs"),
    (4, lambda p: p == "crates/openalpaca_core/src/tools/builtins/update_persona/common.rs"),
    (4, lambda p: p == "crates/openalpaca_core/tests/common/eval.rs"),
    (4, lambda p: p == "apps/openalpacad/src/hot_reload.rs"),
    (4, lambda p: p == "crates/openalpaca_core/src/tools/builtins/memory_tools/tests.rs"),
    (4, lambda p: p == "crates/openalpaca_core/src/tools/builtins/file_edit_tests.rs"),
    (6, lambda p: p.startswith("crates/openalpaca_mcp/")),
    (6, lambda p: p.startswith("crates/openalpaca_core/src/tools/mcp/")),
    (6, lambda p: p == "crates/openalpaca_core/src/tools/config/annotations.rs"),
    (6, lambda p: p.startswith("crates/openalpaca_core/src/tools/registry/")),
    (6, lambda p: p == "crates/openalpaca_core/src/tools/builtins/mod.rs"),
    (6, lambda p: p == "crates/openalpaca_core/src/security/sandbox/tests.rs"),
    (6, lambda p: p == "crates/openalpaca_core/tests/mcp_integration.rs"),
    (6, lambda p: p == "apps/openalpacad/src/routes/tools/tests.rs"),
    (8, lambda p: p.startswith("crates/openalpaca_core/src/middleware/skill/")),
]

# The one file split by hunk (see the module docstring).
SPLIT_BY_HUNK = {"apps/openalpacad/src/managers/mcp.rs": (4, 6)}
DIGEST_MARK = 'format!("{:x}"'
EXPECTED_DIGEST_HUNKS = 11

PATCH_NAME = {4: "step-4-secrets.patch", 6: "step-6-rmcp.patch", 8: "step-8-config.patch"}


def parse(text):
    """Return [(path, header_lines, [hunk_lines...])] in file order."""
    files = []
    cur = None
    for line in text.splitlines(keepends=True):
        if line.startswith("diff --git "):
            m = re.match(r"^diff --git a/(.*) b/(.*)$", line.rstrip("\n"))
            if not m or m.group(1) != m.group(2):
                sys.exit("unsupported diff header: %r" % line)
            cur = {"path": m.group(1), "header": [line], "hunks": []}
            files.append(cur)
        elif cur is None:
            sys.exit("text before the first diff header: %r" % line)
        elif line.startswith("@@ "):
            cur["hunks"].append([line])
        elif cur["hunks"]:
            cur["hunks"][-1].append(line)
        else:
            if line.startswith("index "):
                continue  # dropped on purpose (module docstring)
            cur["header"].append(line)
    return files


def step_for(path):
    for step, pred in STEP_OF_PATH:
        if pred(path):
            return step
    return None


def main(argv):
    if len(argv) != 3:
        sys.exit(__doc__)
    src, out_dir = argv[1], argv[2]
    with open(src, encoding="utf-8") as fh:
        files = parse(fh.read())

    out = {4: [], 6: [], 8: []}
    out_files = {4: [], 6: [], 8: []}
    digest_hunks = 0
    for f in files:
        path = f["path"]
        if path in MANIFESTS:
            continue
        if path in SPLIT_BY_HUNK:
            a, b = SPLIT_BY_HUNK[path]
            for step in (a, b):
                hunks = [h for h in f["hunks"] if (DIGEST_MARK in "".join(h)) == (step == a)]
                if not hunks:
                    sys.exit("%s: no hunk for step %d" % (path, step))
                out[step].append("".join(f["header"]) + "".join("".join(h) for h in hunks))
                out_files[step].append("%s (%d of %d hunks)" % (path, len(hunks), len(f["hunks"])))
                if step == a:
                    digest_hunks += len(hunks)
            continue
        step = step_for(path)
        if step is None:
            sys.exit("unassigned file in the diff: %s" % path)
        out[step].append("".join(f["header"]) + "".join("".join(h) for h in f["hunks"]))
        out_files[step].append(path)
        if step == 4 and not path.startswith("crates/openalpaca_llm/"):
            digest_hunks += sum(1 for h in f["hunks"] if DIGEST_MARK in "".join(h))

    if digest_hunks != EXPECTED_DIGEST_HUNKS:
        sys.exit("expected %d digest hunks in step 4, found %d" % (EXPECTED_DIGEST_HUNKS, digest_hunks))

    os.makedirs(out_dir, exist_ok=True)
    for step, name in sorted(PATCH_NAME.items()):
        target = os.path.join(out_dir, name)
        with open(target, "w", encoding="utf-8") as fh:
            fh.write("".join(out[step]))
        print("%s: %d files" % (name, len(out_files[step])))
        for p in out_files[step]:
            print("  " + p)
    print("manifests left to gbase.sh's edits: %s" % ", ".join(sorted(MANIFESTS)))
    print("digest hunks in step 4: %d" % digest_hunks)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
