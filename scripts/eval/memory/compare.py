#!/usr/bin/env python3
"""Paired comparison of two memory-eval reports (plan §5, "Sample size and the
non-inferiority gate").

    python3 scripts/eval/memory/compare.py report-D.json report-B.json [--margin -5]

The first report is the candidate, the second the baseline. Rows are paired by
question id; a row is counted only when both reports judged it (verdict
`correct` or `incorrect`). Prints each system's accuracy, the paired
difference with its 95 % Wald confidence interval, the discordant-pair rate
(so the power can be checked), the per-category point differences, and the
gate verdict at the given margin in percentage points (−5 against B, −10
against A when B was unavailable — pass `--margin -10`).
"""
import argparse
import json
import math
import sys
from collections import defaultdict


def load(path):
    with open(path, encoding="utf-8") as f:
        report = json.load(f)
    if report.get("status") != "complete":
        print(f"warning: {path} has status {report.get('status')!r}", file=sys.stderr)
    rows = {}
    for r in report.get("rows", []):
        if r.get("judge_verdict") in ("correct", "incorrect"):
            rows[r["id"]] = r
    return report, rows


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("candidate")
    ap.add_argument("baseline")
    ap.add_argument("--margin", type=float, default=-5.0, help="non-inferiority margin in pp (default -5)")
    ap.add_argument("--category-block", type=float, default=-10.0,
                    help="a category whose point estimate is below this blocks (default -10 pp)")
    args = ap.parse_args()

    cand_rep, cand = load(args.candidate)
    base_rep, base = load(args.baseline)
    ids = sorted(set(cand) & set(base))
    if not ids:
        sys.exit("no paired rows: the two reports share no judged question ids")

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

    print(f"candidate: {args.candidate} (system {cand_rep.get('system')})")
    print(f"baseline:  {args.baseline} (system {base_rep.get('system')})")
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
        if d < args.category_block:
            flag = "  <-- below the category block line"
            blocked.append(cat)
        print(f"  {cat:<24} n={cn:<4} {cc/cn*100:5.1f}% vs {cb/cn*100:5.1f}%  {d:+.1f} pp{flag}")

    print()
    passed = lo * 100 > args.margin
    print(f"gate: CI lower bound {lo*100:+.1f} pp {'>' if passed else '<='} margin {args.margin:+.1f} pp -> "
          f"{'PASS' if passed else 'FAIL'}")
    if blocked:
        print(f"blocked categories (need an explanation before release): {', '.join(blocked)}")
    if diff > 0 and lo > 0:
        print("claim allowed: 'more accurate' (positive difference, CI excludes zero)")
    elif passed:
        print("claim allowed: 'comparable measured quality' (non-inferior at the margin)")
    return 0 if passed and not blocked else 1


if __name__ == "__main__":
    sys.exit(main())
