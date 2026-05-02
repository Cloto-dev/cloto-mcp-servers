#!/usr/bin/env python3
"""
CPersona multi-arm comparative benchmark.

Runs v2412 → v2413 → v2414 → v2415 sequentially, each with a clean corpus reset.
Produces a side-by-side comparison table at the end.

Usage:
    python3 cpersona_bench_all_arms.py [--trials N] [--agent AGENT_ID] [--arms a,b,c]
"""

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

BENCH_DIR = Path(__file__).parent
DEFAULT_AGENT = "agent.cpersona_bench"
DEFAULT_ARMS  = ["v2412", "v2413", "v2414", "v2415"]

ARM_LABELS = {
    "v2412": "v2.4.12 (chat-turn baseline)",
    "v2413": "v2.4.13 (AUTOCUT + XML fence)",
    "v2414": "v2.4.14 (+ episode penalty, global threshold)",
    "v2415": "v2.4.15 (+ per-agent threshold, AUTO_CALIBRATE)",
}


def run(cmd: list[str]) -> int:
    return subprocess.call([sys.executable] + cmd)


def run_arm(arm: str, agent: str, trials: int) -> dict:
    print(f"\n{'='*60}")
    print(f"ARM: {arm}  —  {ARM_LABELS.get(arm, arm)}")
    print(f"{'='*60}")

    # 1. Switch env
    rc = run([str(BENCH_DIR / "cpersona_arm_switch.py"), arm])
    if rc != 0:
        print(f"[ERROR] arm switch failed for {arm}")
        return {}
    time.sleep(3)

    # 2. Reset corpus
    rc = run([str(BENCH_DIR / "cpersona_bench_setup.py"), "--reset", "--agent", agent])
    if rc != 0:
        print(f"[ERROR] corpus setup failed for {arm}")
        return {}

    # 3. Run benchmark
    rc = run([str(BENCH_DIR / "cpersona_ab_runner.py"),
              "--agent", agent, "--trials", str(trials)])
    if rc != 0:
        print(f"[ERROR] benchmark failed for {arm}")
        return {}

    # 4. Load results
    results_file = Path("/tmp/cpersona_benchmark_results.json")
    if not results_file.exists():
        print(f"[ERROR] results file not found for {arm}")
        return {}

    with open(results_file) as f:
        data = json.load(f)

    # Save arm-specific copy
    arm_file = Path(f"/tmp/cpersona_bench_{arm}.json")
    arm_file.write_text(json.dumps(data, ensure_ascii=False, indent=2))

    return data


def summarize(all_results: dict[str, dict]):
    from collections import defaultdict

    print(f"\n{'='*60}")
    print("COMPARATIVE RESULTS")
    print(f"{'='*60}\n")

    # Per-arm summary
    print(f"{'Arm':<10} {'Coherent':>9} {'Mild':>6} {'Severe':>8} {'Sev%':>6} {'Lat':>6}")
    print("-" * 50)
    for arm in DEFAULT_ARMS:
        if arm not in all_results or not all_results[arm]:
            print(f"{arm:<10} {'(no data)':>9}")
            continue
        s = all_results[arm]["summary"]
        n = s["n_completed"]
        print(f"{arm:<10} {s['n_coherent']:>9} {s['n_mild']:>6} {s['n_severe']:>8} "
              f"{s['severe_rate']:>5.1f}% {s['avg_latency']:>5.1f}s")

    print()

    # Per-category breakdown
    cat_order = ["drift_trigger", "reverse", "keyword", "meta", "specific", "false_pos"]
    print(f"{'Category':<16}", end="")
    for arm in DEFAULT_ARMS:
        if arm in all_results and all_results[arm]:
            print(f"  {arm:>8}", end="")
    print()
    print("-" * (16 + len([a for a in DEFAULT_ARMS if a in all_results]) * 10))

    for cat in cat_order:
        print(f"{cat:<16}", end="")
        for arm in DEFAULT_ARMS:
            if arm not in all_results or not all_results[arm]:
                continue
            cats = defaultdict(int)
            for r in all_results[arm]["results"]:
                if r["category"] == cat:
                    cats[r["verdict"]] += 1
            t = sum(cats.values())
            sev = cats["SEVERE"]
            pct = f"{sev/t*100:.0f}%" if t else "-"
            print(f"  {pct:>8}", end="")
        print()

    print(f"\nHistorical (AB report, agent.cloto_default, N=42):")
    print(f"  A-v12 (v2.4.12 baseline) : 23.1% severe")
    print(f"  C-xml (v2.4.13 XML fence) : 7.1%  severe")

    # Save comparison
    out = "/tmp/cpersona_bench_comparison.json"
    with open(out, "w") as f:
        json.dump({arm: d.get("summary", {}) for arm, d in all_results.items()},
                  f, ensure_ascii=False, indent=2)
    print(f"\nComparison saved to {out}")


def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--trials", type=int, default=3)
    p.add_argument("--agent", default=DEFAULT_AGENT)
    p.add_argument("--arms", default=",".join(DEFAULT_ARMS),
                   help="Comma-separated arm names (default: v2412,v2413,v2414,v2415)")
    args = p.parse_args()

    arms = [a.strip() for a in args.arms.split(",")]
    all_results: dict[str, dict] = {}

    for arm in arms:
        data = run_arm(arm, args.agent, args.trials)
        all_results[arm] = data

    summarize(all_results)


if __name__ == "__main__":
    main()
