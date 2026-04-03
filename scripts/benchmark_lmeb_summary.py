"""LMEB Benchmark Results Summary.

Aggregates Track A (embedding-only) and Track B (CPersona pipeline) results
into a comparison table.

Usage:
  python scripts/benchmark_lmeb_summary.py
  python scripts/benchmark_lmeb_summary.py --lmeb_dir lmeb_results --cpersona_dir cpersona_lmeb_results
"""

import argparse
import json
import os
import sys

LMEB_TASK_NAMES = [
    "EPBench", "KnowMeBench", "LoCoMo", "LongMemEval", "REALTALK", "TMD",
    "MemBench", "ConvoMem", "QASPER", "NovelQA", "PeerQA", "CovidQA",
    "ESGReports", "MLDR", "LooGLE", "LMEB_SciFact", "Gorilla", "ToolBench",
    "ReMe", "Proced_mem_bench", "MemGovern", "DeepPlanning",
]

MEMORY_TYPES = {
    "EPBench": "Episodic", "KnowMeBench": "Episodic", "ReMe": "Episodic",
    "LoCoMo": "Dialogue", "LongMemEval": "Dialogue", "REALTALK": "Dialogue",
    "TMD": "Dialogue", "MemBench": "Dialogue", "ConvoMem": "Dialogue",
    "QASPER": "Semantic", "NovelQA": "Semantic", "PeerQA": "Semantic",
    "CovidQA": "Semantic", "ESGReports": "Semantic", "MLDR": "Semantic",
    "LooGLE": "Semantic", "LMEB_SciFact": "Semantic",
    "Gorilla": "Procedural", "ToolBench": "Procedural",
    "Proced_mem_bench": "Procedural", "MemGovern": "Procedural",
    "DeepPlanning": "Procedural",
}


def find_model_dirs(base_dir: str) -> dict[str, str]:
    """Find model result directories under base_dir."""
    models = {}
    if not os.path.exists(base_dir):
        return models
    for entry in os.listdir(base_dir):
        path = os.path.join(base_dir, entry)
        if os.path.isdir(path):
            models[entry] = path
    return models


def load_track_a_results(model_dir: str) -> dict[str, float]:
    """Load Track A results from MTEB output structure."""
    results = {}
    for root, dirs, files in os.walk(model_dir):
        for f in files:
            if f.endswith(".json") and f != "model_meta.json" and not f.startswith("_"):
                task_name = f.replace(".json", "")
                fpath = os.path.join(root, f)
                with open(fpath) as fp:
                    data = json.load(fp)
                if "scores" in data:
                    split = list(data["scores"].keys())[0]
                    subsets = data["scores"][split]
                    score = sum(s["main_score"] for s in subsets) / len(subsets)
                    results[task_name] = score
    return results


def load_track_b_results(cpersona_dir: str) -> dict[str, dict]:
    """Load Track B results from CPersona pipeline evaluation."""
    results = {}
    if not os.path.exists(cpersona_dir):
        return results
    for f in os.listdir(cpersona_dir):
        if f.endswith(".json"):
            fpath = os.path.join(cpersona_dir, f)
            with open(fpath) as fp:
                data = json.load(fp)
            task_name = data.get("task", f.replace(".json", ""))
            results[task_name] = data
    return results


def main():
    parser = argparse.ArgumentParser(description="LMEB Results Summary")
    parser.add_argument("--lmeb_dir", default="lmeb_results", help="Track A results dir")
    parser.add_argument("--cpersona_dir", default="cpersona_lmeb_results", help="Track B results dir")
    parser.add_argument("--output", default=None, help="Output JSON path")
    args = parser.parse_args()

    # --- Track A ---
    model_dirs = find_model_dirs(args.lmeb_dir)
    track_a = {}
    for model_name, model_dir in sorted(model_dirs.items()):
        results = load_track_a_results(model_dir)
        if results:
            track_a[model_name] = results

    # --- Track B ---
    track_b = load_track_b_results(args.cpersona_dir)

    if not track_a and not track_b:
        print("No results found.")
        return

    # --- Print comparison table ---
    print("\n" + "=" * 90)
    print("LMEB Benchmark Comparison — NDCG@10")
    print("=" * 90)

    # Header
    header = f"{'Model':<40} {'Track':>5} {'LoCoMo':>8} {'LongMem':>8} {'Dialogue':>9} {'LMEB':>8}"
    print(header)
    print("-" * 90)

    all_results = {}

    # Track A rows
    for model_name, results in sorted(track_a.items()):
        locomo = results.get("LoCoMo", 0)
        longmem = results.get("LongMemEval", 0)

        # Compute type means
        type_scores = {}
        for task, score in results.items():
            mt = MEMORY_TYPES.get(task, "Unknown")
            type_scores.setdefault(mt, []).append(score)

        dialogue_mean = sum(type_scores.get("Dialogue", [0])) / max(len(type_scores.get("Dialogue", [1])), 1)
        overall_mean = sum(results.values()) / len(results) if results else 0

        print(f"{model_name:<40} {'A':>5} {locomo*100:>8.2f} {longmem*100:>8.2f} {dialogue_mean*100:>9.2f} {overall_mean*100:>8.2f}")

        all_results[f"A:{model_name}"] = {
            "track": "A",
            "model": model_name,
            "per_task": results,
            "locomo": locomo,
            "longmemeval": longmem,
            "dialogue_mean": dialogue_mean,
            "overall_mean": overall_mean,
        }

    # Track B rows
    for task_name, data in sorted(track_b.items()):
        subtasks = data.get("subtasks", {})
        mean_ndcg = data.get("mean_ndcg_at_10", 0)

        if task_name == "LoCoMo":
            locomo = mean_ndcg
            longmem = 0
        elif task_name == "LongMemEval":
            locomo = 0
            longmem = mean_ndcg
        else:
            locomo = longmem = 0

        label = f"CPersona ({task_name})"
        print(f"{label:<40} {'B':>5} {locomo*100:>8.2f} {longmem*100:>8.2f} {'n/a':>9} {'n/a':>8}")

        all_results[f"B:{task_name}"] = {
            "track": "B",
            "task": task_name,
            "subtasks": subtasks,
            "mean_ndcg_at_10": mean_ndcg,
        }

    print()

    # --- Per-task detail for Track A ---
    if track_a:
        print("\n" + "=" * 90)
        print("Per-Task NDCG@10 (Track A)")
        print("=" * 90)

        # Column headers
        model_names = sorted(track_a.keys())
        header = f"{'Task':<25} {'Type':<12}"
        for m in model_names:
            short = m[:12]
            header += f" {short:>12}"
        print(header)
        print("-" * 90)

        for task_name in LMEB_TASK_NAMES:
            mt = MEMORY_TYPES.get(task_name, "?")
            row = f"{task_name:<25} {mt:<12}"
            for m in model_names:
                score = track_a[m].get(task_name, None)
                if score is not None:
                    row += f" {score*100:>12.2f}"
                else:
                    row += f" {'—':>12}"
            print(row)

    # --- Save combined JSON ---
    output_path = args.output or os.path.join(args.lmeb_dir, "combined_summary.json")
    with open(output_path, "w") as f:
        json.dump(all_results, f, indent=2, default=str)
    print(f"\nCombined results saved to {output_path}")


if __name__ == "__main__":
    main()
