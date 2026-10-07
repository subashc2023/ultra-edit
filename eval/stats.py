#!/usr/bin/env python3
"""Paired, task-balanced comparisons between arms from runs.jsonl.

    python3 eval/stats.py RESULTS_DIR[:SUFFIX] [RESULTS_DIR[:SUFFIX] ...] > stats.md

Several results dirs are pooled (for example a pilot plus a later run). A
`:SUFFIX` renames that directory's arms (`ultra-edit` -> `ultra-edit-SUFFIX`), so
two builds of one arm can be compared side by side.
Only scored runs (pass/fail/timeout) count. For each arm versus a baseline arm,
per-task means are compared as ratios, and the geometric mean of those ratios
across tasks gets a bootstrap interval (resampling repetitions within each
task, 2,000 draws, fixed seed). That keeps one expensive task from dominating
and pairs each arm with the baseline on the same tasks.
"""

import json
import math
import random
import statistics
import sys
from collections import defaultdict
from pathlib import Path

SCORED = ("pass", "fail", "timeout")
METRICS = ("cost_usd", "context_tokens_total", "output_tokens", "turns", "tool_calls", "api_calls")
BASELINES = ("native", "ultra-edit")


def load(specs):
    records = []
    for spec in specs:
        directory, _, suffix = spec.partition(":")
        for line in (Path(directory) / "runs.jsonl").read_text(encoding="utf-8").splitlines():
            if line.strip():
                r = json.loads(line)
                r["_dir"] = directory
                if suffix:
                    r["arm"] = f"{r['arm']}-{suffix}"
                records.append(r)
    return records


def metric(r, key):
    m = r.get("metrics", {})
    v = m.get(key, r.get(key))
    return float(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else None


def wilson(k, n, z=1.96):
    if n == 0:
        return (float("nan"), float("nan"))
    p = k / n
    d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    h = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return (c - h, c + h)


def geo_ratio(arm_runs, base_runs, key, rng=None):
    ratios = []
    for task in sorted(set(arm_runs) & set(base_runs)):
        a = [metric(r, key) for r in arm_runs[task]]
        b = [metric(r, key) for r in base_runs[task]]
        a = [x for x in a if x is not None]
        b = [x for x in b if x is not None]
        if not a or not b:
            continue
        if rng is not None:
            a = [rng.choice(a) for _ in a]
            b = [rng.choice(b) for _ in b]
        ma, mb = statistics.mean(a), statistics.mean(b)
        if ma > 0 and mb > 0:
            ratios.append(math.log(ma / mb))
    if not ratios:
        return None
    return math.exp(statistics.mean(ratios))


def report(specs):
    records = [r for r in load(specs) if r.get("outcome") in SCORED]
    by = defaultdict(lambda: defaultdict(list))
    for r in records:
        by[r["arm"]][r["task"]].append(r)
    arms = sorted(by)
    out = []
    out.append(
        f"# Paired benchmark statistics\n\n{len(records)} scored runs from {len(specs)} results dir(s).\n"
    )
    out.append("## Correctness\n")
    out.append("| Arm | Runs | Correct | 95% CI | First try | 95% CI | Tasks always correct |")
    out.append("| --- | --- | --- | --- | --- | --- | --- |")
    for arm in arms:
        runs = [r for t in by[arm].values() for r in t]
        n = len(runs)
        k = sum(1 for r in runs if r.get("correct"))
        f = sum(1 for r in runs if r.get("first_attempt"))
        always = sum(1 for t in by[arm].values() if all(r.get("correct") for r in t))
        lo, hi = wilson(k, n)
        flo, fhi = wilson(f, n)
        out.append(
            f"| {arm} | {n} | {k}/{n} ({k / n:.0%}) | {lo:.0%}-{hi:.0%} | {f}/{n} ({f / n:.0%}) | {flo:.0%}-{fhi:.0%} | {always}/{len(by[arm])} |"
        )
    rng = random.Random(20261007)
    for base in BASELINES:
        if base not in by:
            continue
        out.append(f"\n## Versus {base} (geometric mean of per-task ratios, 95% bootstrap interval)\n")
        out.append("Below 1.00 means the arm used less than " + base + ".\n")
        out.append("| Arm | " + " | ".join(METRICS) + " |")
        out.append("| --- |" + " --- |" * len(METRICS))
        for arm in arms:
            if arm == base:
                continue
            cells = []
            for key in METRICS:
                point = geo_ratio(by[arm], by[base], key)
                if point is None:
                    cells.append("-")
                    continue
                draws = sorted(
                    x for x in (geo_ratio(by[arm], by[base], key, rng) for _ in range(2000)) if x is not None
                )
                lo, hi = draws[int(0.025 * len(draws))], draws[int(0.975 * len(draws)) - 1]
                cells.append(f"{point:.2f} ({lo:.2f}-{hi:.2f})")
            out.append(f"| {arm} | " + " | ".join(cells) + " |")
    out.append("\n## Per task: mean cost (USD) and correct runs\n")
    tasks = sorted({t for a in by.values() for t in a})
    out.append("| Task | " + " | ".join(arms) + " |")
    out.append("| --- |" + " --- |" * len(arms))
    for task in tasks:
        cells = []
        for arm in arms:
            runs = by[arm].get(task, [])
            if not runs:
                cells.append("-")
                continue
            costs = [metric(r, "cost_usd") for r in runs if metric(r, "cost_usd") is not None]
            k = sum(1 for r in runs if r.get("correct"))
            cells.append(
                f"{statistics.mean(costs):.3f} ({k}/{len(runs)})" if costs else f"- ({k}/{len(runs)})"
            )
        out.append(f"| {task} | " + " | ".join(cells) + " |")
    out.append("\n## Per task: mean context tokens (thousands)\n")
    out.append("| Task | " + " | ".join(arms) + " |")
    out.append("| --- |" + " --- |" * len(arms))
    for task in tasks:
        cells = []
        for arm in arms:
            vals = [metric(r, "context_tokens_total") for r in by[arm].get(task, [])]
            vals = [v for v in vals if v is not None]
            cells.append(f"{statistics.mean(vals) / 1000:.0f}" if vals else "-")
        out.append(f"| {task} | " + " | ".join(cells) + " |")
    out.append("\n## Variability across repetitions (coefficient of variation of cost, mean over tasks)\n")
    out.append("| Arm | CV cost | CV context | Max/min cost ratio within a task (median) |")
    out.append("| --- | --- | --- | --- |")
    for arm in arms:
        cvs, cvc, spreads = [], [], []
        for runs in by[arm].values():
            costs = [metric(r, "cost_usd") for r in runs if metric(r, "cost_usd")]
            ctx = [metric(r, "context_tokens_total") for r in runs if metric(r, "context_tokens_total")]
            if len(costs) >= 2:
                cvs.append(statistics.stdev(costs) / statistics.mean(costs))
                spreads.append(max(costs) / min(costs))
            if len(ctx) >= 2:
                cvc.append(statistics.stdev(ctx) / statistics.mean(ctx))
        fmt = lambda xs: f"{statistics.mean(xs):.2f}" if xs else "-"
        med = f"{statistics.median(spreads):.2f}" if spreads else "-"
        out.append(f"| {arm} | {fmt(cvs)} | {fmt(cvc)} | {med} |")
    return "\n".join(out) + "\n"


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    sys.stdout.write(report(sys.argv[1:]))


if __name__ == "__main__":
    main()
