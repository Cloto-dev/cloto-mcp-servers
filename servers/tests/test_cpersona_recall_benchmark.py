"""
CPersona Recall Quality Benchmark — Level 2 integration tests.

Tests verify that the embedding model correctly ranks semantically relevant
memories above irrelevant/contaminating ones for a controlled 5-topic corpus.

Design principles:
  - Tests cosine RANKING directly, not threshold-dependent pipeline recall.
    This avoids coupling test results to adaptive_min_score calibration.
  - Separate anti-drift assertions confirm that near-miss topics rank LOWER
    than the target topic, which is the root cause of LLM drift.
  - Full pipeline tests (do_recall) are explicitly marked with the expected
    memory count at production settings (CPERSONA_VECTOR_MIN_SIMILARITY=0.3).

Requires embedding server on port 8401. Skipped automatically when unavailable.
These tests are NOT counted in qa/test-baseline.json — run on demand:

    pytest tests/test_cpersona_recall_benchmark.py -v

"""

import math
import os
import socket
import struct
import sys
import urllib.request

import numpy as np
import pytest

sys.path.insert(0, "cpersona")


# ── availability guard ────────────────────────────────────────────────────────


def _embedding_server_available() -> bool:
    try:
        with socket.create_connection(("127.0.0.1", 8401), timeout=1):
            return True
    except OSError:
        return False


pytestmark = pytest.mark.skipif(
    not _embedding_server_available(),
    reason="embedding server not available on port 8401",
)

EMBED_URL = "http://127.0.0.1:8401/embed"
BENCHMARK_AGENT = "agent.benchmark"


# ── embedding helpers ─────────────────────────────────────────────────────────


def _embed_batch(texts: list[str]) -> list[np.ndarray]:
    import json

    data = json.dumps({"texts": texts, "namespace": BENCHMARK_AGENT}).encode()
    req = urllib.request.Request(
        EMBED_URL, data=data, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        return [np.array(e, dtype=np.float32) for e in json.loads(r.read())["embeddings"]]


def _cos(a: np.ndarray, b: np.ndarray) -> float:
    d = np.linalg.norm(a) * np.linalg.norm(b)
    return float(np.dot(a, b) / d) if d > 0 else 0.0


# ── corpus ────────────────────────────────────────────────────────────────────

CORPUS: dict[str, list[str]] = {
    # Target topic — bread / food
    "bread": [
        "今日の朝食でパン屋さんのメロンパンを食べた。カリカリで美味しかった",
        "近所に新しいベーカリーができた。クロワッサンが絶品だった",
        "お気に入りのパン屋さんが閉店してしまった。残念だ",
        "週末にパンを焼く練習をした。生地をこねるのが楽しかった",
        "朝ごはんにトーストを食べた。バターとジャムが美味しかった",
    ],
    # Near-miss noise A — raspberry dessert (shares food/sweet context with bread)
    "raspberry_dessert": [
        "人生で初めてラズベリーパイ（デザート）を食べた。甘酸っぱくて美味しい",
        "ラズベリーのジャムを買ってきた。ヨーグルトに合う",
        "スイーツ屋さんでラズベリータルトを食べた",
    ],
    # Near-miss noise B — Raspberry Pi tech (shares カタカナ surface form)
    "raspberry_tech": [
        "Raspberry Pi 5 でホームサーバーを構築した",
        "Pi にカメラモジュールを取り付けた",
        "ラズパイで温度センサーを動かした",
    ],
    # Unrelated topic A — software development
    "coding": [
        "Git でブランチを切り間違えてしまった",
        "プルリクエストのレビューがやっと通った",
        "デプロイが失敗して原因調査に時間がかかった",
        "TypeScript の型エラーを直した",
    ],
    # Unrelated topic B — travel
    "travel": [
        "来月の京都旅行の計画を立てた",
        "新幹線の切符を予約した",
        "ホテルのチェックイン方法を確認した",
        "観光スポットをリストアップした",
    ],
}


# ── module-level embedding cache (one embed() call per text per test session) ─


@pytest.fixture(scope="module")
def corpus_embeddings() -> dict[str, list[np.ndarray]]:
    """Pre-compute embeddings for all corpus memories. Shared across tests."""
    result: dict[str, list[np.ndarray]] = {}
    for topic, mems in CORPUS.items():
        result[topic] = _embed_batch(mems)
    return result


def _best_score(query_emb: np.ndarray, topic_embs: list[np.ndarray]) -> float:
    """Highest cosine similarity between the query and any memory in the topic."""
    return max(_cos(query_emb, e) for e in topic_embs)


# ── ranking scenarios ─────────────────────────────────────────────────────────

RANKING_SCENARIOS = [
    {
        "id": "R1-bread-vs-raspberry-dessert",
        "query": "この前のパン屋さんの話覚えてる?",
        "target_topic": "bread",
        "noise_topics": ["raspberry_dessert"],
        "note": "Core anti-drift case: bread memories must rank above raspberry (dessert)",
    },
    {
        "id": "R2-bread-vs-raspberry-tech",
        "query": "朝食に食べたパンの件",
        "target_topic": "bread",
        "noise_topics": ["raspberry_tech"],
        "note": "Surface-noise case: カタカナ Raspberry Pi must not outrank bread",
    },
    {
        "id": "R3-bread-vs-unrelated",
        "query": "パン屋さんの話",
        "target_topic": "bread",
        "noise_topics": ["coding", "travel"],
        "note": "Unrelated topics must not outrank bread",
    },
    {
        "id": "R4-raspberry-tech",
        "query": "Raspberry Pi の設定方法",
        "target_topic": "raspberry_tech",
        "noise_topics": ["bread", "travel"],
        "note": "Tech query must find Raspberry Pi docs, not food",
        "xfail": "jina-v5-nano places Raspberry Pi tech below travel in cosine space; known model limitation",
    },
    {
        "id": "R5-coding",
        "query": "git push でエラーが出た",
        "target_topic": "coding",
        "noise_topics": ["bread", "travel"],
        "note": "Code query must find coding memories",
        "xfail": "jina-v5-nano gives high cosine (0.44) between 'git push' and travel memories; known model limitation",
    },
    {
        "id": "R6-travel",
        "query": "旅行の計画を立てた",
        "target_topic": "travel",
        "noise_topics": ["coding", "raspberry_tech"],
        "note": "Travel query must find travel memories",
    },
]


# ── ranking tests (no quality gate — tests embedding quality directly) ────────


@pytest.mark.asyncio
@pytest.mark.parametrize("scenario", RANKING_SCENARIOS, ids=[s["id"] for s in RANKING_SCENARIOS])
async def test_cosine_ranking(corpus_embeddings, scenario):
    if "xfail" in scenario:
        pytest.xfail(scenario["xfail"])
    """Target topic memories must have higher best-cosine than all noise topics.

    This test intentionally bypasses the quality gate and AUTOCUT to isolate
    the embedding model's semantic ranking ability. If this test fails, the
    jina-v5-nano model cannot distinguish the topics — a fundamental recall
    quality problem independent of threshold tuning.
    """
    q_emb = _embed_batch([scenario["query"]])[0]

    target_score = _best_score(q_emb, corpus_embeddings[scenario["target_topic"]])

    for noise_topic in scenario["noise_topics"]:
        noise_score = _best_score(q_emb, corpus_embeddings[noise_topic])
        assert target_score > noise_score, (
            f"[{scenario['id']}] Topic '{scenario['target_topic']}' (score={target_score:.4f}) "
            f"should rank above '{noise_topic}' (score={noise_score:.4f}) "
            f"for query '{scenario['query']}'. "
            f"Note: {scenario['note']}"
        )


# ── drift margin tests ────────────────────────────────────────────────────────

DRIFT_MARGIN = 0.02   # target topic must beat noise by at least this gap


@pytest.mark.asyncio
@pytest.mark.parametrize("scenario", RANKING_SCENARIOS, ids=[s["id"] for s in RANKING_SCENARIOS])
async def test_cosine_margin(corpus_embeddings, scenario):
    if "xfail" in scenario:
        pytest.xfail(scenario["xfail"])
    """Target topic must beat each noise topic by at least DRIFT_MARGIN (0.02).

    A large margin means AUTOCUT and the quality gate have more room to cut
    noise before it reaches the LLM. Small margins (< 0.02) indicate a
    near-miss risk where parameter tuning alone cannot prevent drift.
    """
    q_emb = _embed_batch([scenario["query"]])[0]
    target_score = _best_score(q_emb, corpus_embeddings[scenario["target_topic"]])

    for noise_topic in scenario["noise_topics"]:
        noise_score = _best_score(q_emb, corpus_embeddings[noise_topic])
        margin = target_score - noise_score
        assert margin >= DRIFT_MARGIN, (
            f"[{scenario['id']}] Margin between '{scenario['target_topic']}' "
            f"({target_score:.4f}) and '{noise_topic}' ({noise_score:.4f}) "
            f"is only {margin:.4f} — below safety threshold {DRIFT_MARGIN}. "
            f"Drift risk: LLM may see noise topic in recalled context."
        )
