"""
Embedding model comparison benchmark for CPersona memory drift.

Directly calls ONNX providers (no HTTP server needed).
Tests R1-R6 scenarios from test_cpersona_recall_benchmark.py.

Usage:
    cd ClotoCore/target/debug/data/mcp-servers
    python3 /path/to/cpersona_embedding_compare.py [--models jina bge-m3]
"""

import argparse
import asyncio
import os
import sys

import numpy as np

# --- corpus (same as test_cpersona_recall_benchmark.py) ---

CORPUS = {
    "bread": [
        "今日の朝食でパン屋さんのメロンパンを食べた。カリカリで美味しかった",
        "近所に新しいベーカリーができた。クロワッサンが絶品だった",
        "お気に入りのパン屋さんが閉店してしまった。残念だ",
        "週末にパンを焼く練習をした。生地をこねるのが楽しかった",
        "朝ごはんにトーストを食べた。バターとジャムが美味しかった",
    ],
    "raspberry_dessert": [
        "人生で初めてラズベリーパイ（デザート）を食べた。甘酸っぱくて美味しい",
        "ラズベリーのジャムを買ってきた。ヨーグルトに合う",
        "スイーツ屋さんでラズベリータルトを食べた",
    ],
    "raspberry_tech": [
        "Raspberry Pi 5 でホームサーバーを構築した",
        "Pi にカメラモジュールを取り付けた",
        "ラズパイで温度センサーを動かした",
    ],
    "coding": [
        "Git でブランチを切り間違えてしまった",
        "プルリクエストのレビューがやっと通った",
        "デプロイが失敗して原因調査に時間がかかった",
        "TypeScript の型エラーを直した",
    ],
    "travel": [
        "来月の京都旅行の計画を立てた",
        "新幹線の切符を予約した",
        "ホテルのチェックイン方法を確認した",
        "観光スポットをリストアップした",
    ],
}

SCENARIOS = [
    {
        "id": "R1-bread-vs-raspberry-dessert",
        "query": "この前のパン屋さんの話覚えてる?",
        "target": "bread",
        "noise": ["raspberry_dessert"],
        "xfail_models": [],
    },
    {
        "id": "R2-bread-vs-raspberry-tech",
        "query": "朝食に食べたパンの件",
        "target": "bread",
        "noise": ["raspberry_tech"],
        "xfail_models": [],
    },
    {
        "id": "R3-bread-vs-unrelated",
        "query": "パン屋さんの話",
        "target": "bread",
        "noise": ["coding", "travel"],
        "xfail_models": [],
    },
    {
        "id": "R4-raspberry-tech",
        "query": "Raspberry Pi の設定方法",
        "target": "raspberry_tech",
        "noise": ["bread", "travel"],
        "xfail_models": ["jina"],  # known jina-v5-nano limitation
    },
    {
        "id": "R5-coding",
        "query": "git push でエラーが出た",
        "target": "coding",
        "noise": ["bread", "travel"],
        "xfail_models": ["jina"],  # known jina-v5-nano limitation
    },
    {
        "id": "R6-travel",
        "query": "旅行の計画を立てた",
        "target": "travel",
        "noise": ["coding", "raspberry_tech"],
        "xfail_models": [],
    },
]

DRIFT_MARGIN = 0.02


def _cos(a: np.ndarray, b: np.ndarray) -> float:
    d = np.linalg.norm(a) * np.linalg.norm(b)
    return float(np.dot(a, b) / d) if d > 0 else 0.0


def _best(q: np.ndarray, embs: list[np.ndarray]) -> float:
    return max(_cos(q, e) for e in embs)


async def run_benchmark(provider, model_name: str):
    print(f"\n{'='*60}")
    print(f"Model: {model_name}  ({provider.dimensions()}-dim)")
    print(f"{'='*60}")

    # Pre-compute all corpus embeddings
    all_texts = {topic: texts for topic, texts in CORPUS.items()}
    corpus_embs: dict[str, list[np.ndarray]] = {}
    for topic, texts in all_texts.items():
        raw = await provider.embed(texts)
        corpus_embs[topic] = [np.array(e) for e in raw]

    results = []
    pass_count = 0
    xfail_count = 0
    fail_count = 0

    for sc in SCENARIOS:
        q_raw = await provider.embed([sc["query"]])
        q = np.array(q_raw[0])

        target_score = _best(q, corpus_embs[sc["target"]])

        scenario_ok = True
        worst_margin = float("inf")
        details = []
        for noise_topic in sc["noise"]:
            noise_score = _best(q, corpus_embs[noise_topic])
            margin = target_score - noise_score
            worst_margin = min(worst_margin, margin)
            details.append(
                f"  {sc['target']}({target_score:.4f}) vs {noise_topic}({noise_score:.4f})"
                f"  margin={margin:+.4f}"
            )
            if margin < DRIFT_MARGIN:
                scenario_ok = False

        is_xfail = model_name in sc.get("xfail_models", [])
        if scenario_ok:
            status = "PASS"
            if is_xfail:
                status = "XPASS"  # unexpected pass — model improved!
                xfail_count += 1
            else:
                pass_count += 1
        else:
            if is_xfail:
                status = "xfail"
                xfail_count += 1
            else:
                status = "FAIL"
                fail_count += 1

        print(f"\n[{status}] {sc['id']}")
        print(f"  query: {sc['query']}")
        for d in details:
            print(d)

        results.append(
            {
                "id": sc["id"],
                "status": status,
                "worst_margin": worst_margin,
                "is_xfail": is_xfail,
            }
        )

    print(f"\n{'─'*60}")
    print(f"Summary: PASS={pass_count}  FAIL={fail_count}  xfail/XPASS={xfail_count}")
    return results


async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--models",
        nargs="+",
        default=["jina", "bge-m3"],
        choices=["jina", "bge-m3"],
    )
    args = parser.parse_args()

    # CWD must be the mcp-servers root (where data/models/ lives)
    script_dir = os.path.dirname(os.path.abspath(__file__))
    mcp_root = os.path.normpath(os.path.join(script_dir, "../../servers"))
    # Try the ClotoCore target path
    clotocore_target = os.path.normpath(
        os.path.join(script_dir, "../../../ClotoCore/target/debug/data/mcp-servers")
    )
    for candidate in [clotocore_target, os.path.join(script_dir, "../../../target/debug/data/mcp-servers")]:
        if os.path.isdir(os.path.join(candidate, "data/models")):
            mcp_root = candidate
            break

    # Allow explicit override via CWD if data/models already present
    if os.path.isdir("data/models"):
        mcp_root = os.getcwd()

    print(f"[bench] mcp_root: {mcp_root}")
    os.chdir(mcp_root)
    sys.path.insert(0, "embedding")
    os.environ.setdefault("ONNX_MAX_SEQ_LEN", "512")

    from server import OnnxBgeM3Provider, OnnxJinaV5NanoProvider

    all_results: dict[str, list] = {}

    for model in args.models:
        if model == "jina":
            provider = OnnxJinaV5NanoProvider(model_dir="data/models/jina-embeddings-v5-text-nano")
        else:
            provider = OnnxBgeM3Provider(model_dir="data/models/bge-m3")
        await provider.initialize()
        all_results[model] = await run_benchmark(provider, model)
        await provider.shutdown()

    # Side-by-side comparison
    if len(args.models) == 2:
        m1, m2 = args.models
        r1 = {r["id"]: r for r in all_results[m1]}
        r2 = {r["id"]: r for r in all_results[m2]}
        print(f"\n{'='*60}")
        print(f"Comparison: {m1} → {m2}")
        print(f"{'='*60}")
        for sc in SCENARIOS:
            sid = sc["id"]
            s1 = r1[sid]["status"]
            s2 = r2[sid]["status"]
            m_delta = r2[sid]["worst_margin"] - r1[sid]["worst_margin"]
            arrow = "→"
            if s1 != s2:
                arrow = "🔼" if s2 in ("PASS", "XPASS") and s1 not in ("PASS", "XPASS") else "🔽"
            print(f"  {sid}: {s1} {arrow} {s2}  (margin delta: {m_delta:+.4f})")


if __name__ == "__main__":
    asyncio.run(main())
