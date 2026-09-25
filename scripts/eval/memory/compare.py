#!/usr/bin/env python3
"""Paired comparison of two memory-eval reports (plan §5, "Sample size and the
non-inferiority gate").

    python3 scripts/eval/memory/compare.py report-D.json report-B.json [--margin -5]
    python3 scripts/eval/memory/compare.py --self-test

The first report is the candidate, the second the baseline. Rows are paired by
question id; a row is counted only when both reports judged it (verdict
`correct` or `incorrect`). Prints each system's accuracy, the paired
difference with its 95 % Wald confidence interval, the discordant-pair rate
(so the power can be checked), the per-category point differences, and the
gate verdict at the given margin in percentage points (−5 against B, −10
against A when B was unavailable — pass `--margin -10`).

Both reports' judge models are printed first — the models that answered the
judge calls of the paired rows — and two reports graded by different judges
are refused (design 09 §7.4): a difference between them would be partly the
judges'. `--allow-judge-mismatch` compares them anyway and says so. A report
that records `judge_is_reader` (the reader graded its own answers, owner
answer LQ1) has it printed beside its judge, and one that records
`recall_mode` (`index` or `excerpts`, design 09 §13.2) has that printed too —
two reports in different recall modes are compared, never refused: that is
L8's D-excerpts against D-index (design 09 §13.4).
"""
import argparse
import io
import json
import math
import os
import sys
import tempfile
from collections import defaultdict

JUDGED = ("correct", "incorrect")


def load(path):
    with open(path, encoding="utf-8") as f:
        report = json.load(f)
    if report.get("status") != "complete":
        print(f"warning: {path} has status {report.get('status')!r}", file=sys.stderr)
    rows = {}
    for r in report.get("rows", []):
        if r.get("judge_verdict") in JUDGED:
            rows[r["id"]] = r
    return report, rows


def judge_models(rows, ids):
    """The models that answered the judge calls of these rows."""
    return sorted({rows[i]["judge_model"] for i in ids if rows[i].get("judge_model")})


def judge_line(report, models):
    conditions = report.get("conditions") or {}
    line = ", ".join(models) if models else "(none recorded)"
    if conditions.get("judge_is_reader") is not None:
        line += f"  judge_is_reader={str(conditions['judge_is_reader']).lower()}"
    if conditions.get("judge_same_family") is not None:
        line += f"  judge_same_family={str(conditions['judge_same_family']).lower()}"
    # Printed, never checked: D-excerpts against D-index is a comparison
    # worth making (design 09 §13.4). The archived reports record no mode.
    if conditions.get("recall_mode") is not None:
        line += f"  recall_mode={conditions['recall_mode']}"
    return line


def judge_check(cand_models, base_models, allow_mismatch):
    """`None` when the two may be compared, else the refusal."""
    if cand_models == base_models:
        return None
    message = (
        f"the reports were graded by different judges: candidate {cand_models or '(none)'}, "
        f"baseline {base_models or '(none)'}. Re-judge one with the other's judge "
        "(OPENALPACA_MEMORY_EVAL_REJUDGE), or pass --allow-judge-mismatch to compare them anyway."
    )
    if allow_mismatch:
        print(f"warning: {message}", file=sys.stderr)
        return None
    return message


def compare(candidate, baseline, margin=-5.0, category_block=-10.0, allow_judge_mismatch=False):
    cand_rep, cand = load(candidate)
    base_rep, base = load(baseline)
    ids = sorted(set(cand) & set(base))
    if not ids:
        sys.exit("no paired rows: the two reports share no judged question ids")

    cand_judges = judge_models(cand, ids)
    base_judges = judge_models(base, ids)
    print(f"candidate judge: {judge_line(cand_rep, cand_judges)}")
    print(f"baseline judge:  {judge_line(base_rep, base_judges)}")
    refusal = judge_check(cand_judges, base_judges, allow_judge_mismatch)
    if refusal:
        sys.exit(f"refused: {refusal}")
    cand_prompt = (cand_rep.get("conditions") or {}).get("judge_prompt_sha256")
    base_prompt = (base_rep.get("conditions") or {}).get("judge_prompt_sha256")
    if cand_prompt and base_prompt and cand_prompt != base_prompt:
        print("warning: the two reports used different judge prompts (judge_prompt_sha256)", file=sys.stderr)

    n = len(ids)
    b01 = sum(1 for i in ids if not cand[i]["correct"] and base[i]["correct"])  # baseline only
    b10 = sum(1 for i in ids if cand[i]["correct"] and not base[i]["correct"])  # candidate only
    both = sum(1 for i in ids if cand[i]["correct"] and base[i]["correct"])
    acc_c = sum(1 for i in ids if cand[i]["correct"]) / n
    acc_b = sum(1 for i in ids if base[i]["correct"]) / n
    diff = acc_c - acc_b
    # Wald SE of a paired difference of proportions.
    se = math.sqrt(max(b01 + b10 - (b10 - b01) ** 2 / n, 0.0)) / n
    lo, hi = diff - 1.96 * se, diff + 1.96 * se
    discordant = (b01 + b10) / n

    print(f"candidate: {candidate} (system {cand_rep.get('system')})")
    print(f"baseline:  {baseline} (system {base_rep.get('system')})")
    print(f"paired questions: {n}  (candidate judged {len(cand)}, baseline judged {len(base)})")
    print(f"accuracy: candidate {acc_c*100:.1f}%  baseline {acc_b*100:.1f}%")
    print(f"paired difference: {diff*100:+.1f} pp  95% CI [{lo*100:+.1f}, {hi*100:+.1f}]  SE {se*100:.2f} pp")
    print(f"discordant pairs: {b01 + b10} ({discordant*100:.1f}%): candidate-only {b10}, baseline-only {b01}, both {both}")

    cats = defaultdict(lambda: [0, 0, 0])  # n, cand correct, base correct
    for i in ids:
        c = cats[cand[i]["category"]]
        c[0] += 1
        c[1] += int(bool(cand[i]["correct"]))
        c[2] += int(bool(base[i]["correct"]))
    print("\nper category (candidate vs baseline, point difference):")
    blocked = []
    for cat in sorted(cats):
        cn, cc, cb = cats[cat]
        d = (cc - cb) / cn * 100
        flag = ""
        if d < category_block:
            flag = "  <-- below the category block line"
            blocked.append(cat)
        print(f"  {cat:<24} n={cn:<4} {cc/cn*100:5.1f}% vs {cb/cn*100:5.1f}%  {d:+.1f} pp{flag}")

    print()
    passed = lo * 100 > margin
    print(f"gate: CI lower bound {lo*100:+.1f} pp {'>' if passed else '<='} margin {margin:+.1f} pp -> "
          f"{'PASS' if passed else 'FAIL'}")
    if blocked:
        print(f"blocked categories (need an explanation before release): {', '.join(blocked)}")
    if diff > 0 and lo > 0:
        print("claim allowed: 'more accurate' (positive difference, CI excludes zero)")
    elif passed:
        print("claim allowed: 'comparable measured quality' (non-inferior at the margin)")
    return 0 if passed and not blocked else 1


def self_test():
    """The judge check on synthetic reports: the same judge compares, a
    different judge is refused, and --allow-judge-mismatch compares anyway;
    and each report's recall mode is printed, a mismatch compared."""

    def report(system, judge, judge_is_reader=None, recall_mode=None):
        rows = []
        for i in range(20):
            correct = i % 4 != 0
            rows.append({
                "id": f"q{i:02d}", "category": "exact_recall", "correct": correct,
                "judge_verdict": "correct" if correct else "incorrect", "judge_model": judge,
            })
        conditions = {"judge_prompt_sha256": "0" * 64}
        if judge_is_reader is not None:
            conditions["judge_is_reader"] = judge_is_reader
        if recall_mode is not None:
            conditions["recall_mode"] = recall_mode
        return {"schema": 1, "status": "complete", "system": system, "conditions": conditions, "rows": rows}

    def run_printing(cand, base, **kw):
        """The comparison's result and what it printed."""
        with tempfile.TemporaryDirectory() as d:
            paths = []
            for name, rep in (("cand.json", cand), ("base.json", base)):
                p = os.path.join(d, name)
                with open(p, "w", encoding="utf-8") as f:
                    json.dump(rep, f)
                paths.append(p)
            stdout = sys.stdout
            sys.stdout = io.StringIO()
            try:
                code = compare(paths[0], paths[1], **kw)
            except SystemExit as e:
                code = e.code
            finally:
                printed = sys.stdout.getvalue()
                sys.stdout = stdout
            return code, printed

    def run(cand, base, **kw):
        return run_printing(cand, base, **kw)[0]

    failures = []

    def expect(label, got, want):
        ok = want(got) if callable(want) else got == want
        print(f"  {'ok  ' if ok else 'FAIL'} {label}: {got!r}")
        if not ok:
            failures.append(label)

    same = run(report("L", "qwen3.8:27b-mlx", True), report("D", "qwen3.8:27b-mlx", True))
    expect("the same judge is compared", same, 0)
    refused = run(report("L", "qwen3.8:27b-mlx", True), report("B", "qwen3.8:27b-mtp-q8_0"))
    expect("a different judge is refused", refused,
           lambda r: isinstance(r, str) and r.startswith("refused: the reports were graded by different judges"))
    allowed = run(report("L", "qwen3.8:27b-mlx", True), report("B", "qwen3.8:27b-mtp-q8_0"),
                  allow_judge_mismatch=True)
    expect("--allow-judge-mismatch compares anyway", allowed, 0)
    unrecorded = run(report("L", "qwen3.8:27b-mlx"), report("A", None))
    expect("a report with no recorded judge is refused against one with", unrecorded,
           lambda r: isinstance(r, str) and r.startswith("refused:"))
    # Design 09 §13.4: D-excerpts against D-index is the comparison L8 makes,
    # so a recall-mode mismatch is printed on each header line, never refused.
    code, printed = run_printing(report("D", "qwen3.8:27b-mlx", True, "excerpts"),
                                 report("D", "qwen3.8:27b-mlx", True, "index"))
    expect("two recall modes are compared", code, 0)
    header = dict(line.split(":", 1) for line in printed.splitlines()[:2])
    expect("the candidate's recall mode is printed beside its judge", header.get("candidate judge", ""),
           lambda line: line.endswith("judge_is_reader=true  recall_mode=excerpts"))
    expect("the baseline's recall mode is printed beside its judge", header.get("baseline judge", ""),
           lambda line: line.endswith("recall_mode=index"))
    code, printed = run_printing(report("D", "qwen3.8:27b-mtp-q8_0", None, "index"),
                                 report("B", "qwen3.8:27b-mtp-q8_0"))
    expect("an archived report, which records no mode, is compared", code, 0)
    expect("and prints none", printed.splitlines()[1],
           lambda line: line.startswith("baseline judge:") and "recall_mode" not in line)
    print("self-test:", "FAILED" if failures else "passed")
    return 1 if failures else 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("candidate", nargs="?")
    ap.add_argument("baseline", nargs="?")
    ap.add_argument("--margin", type=float, default=-5.0, help="non-inferiority margin in pp (default -5)")
    ap.add_argument("--category-block", type=float, default=-10.0,
                    help="a category whose point estimate is below this blocks (default -10 pp)")
    ap.add_argument("--allow-judge-mismatch", action="store_true",
                    help="compare two reports graded by different judge models (refused by default)")
    ap.add_argument("--self-test", action="store_true",
                    help="check the judge-mismatch refusal and the recall-mode printout, and exit")
    args = ap.parse_args()
    if args.self_test:
        return self_test()
    if not args.candidate or not args.baseline:
        ap.error("the candidate and baseline reports are required")
    return compare(args.candidate, args.baseline, args.margin, args.category_block, args.allow_judge_mismatch)


if __name__ == "__main__":
    sys.exit(main())
