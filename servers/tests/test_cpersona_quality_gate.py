"""Tests for CPersona v2.4.6 Adaptive Quality Gate."""

import sys

sys.path.insert(0, "cpersona")

from server import _adaptive_min_score, _apply_quality_gate  # noqa: E402, I001


# ── _adaptive_min_score ──


def test_adaptive_min_score_zero():
    assert _adaptive_min_score(0) == 1.0


def test_adaptive_min_score_sparse():
    score = _adaptive_min_score(5)
    assert 0.40 < score < 0.45


def test_adaptive_min_score_medium():
    score = _adaptive_min_score(50)
    assert 0.30 < score < 0.36


def test_adaptive_min_score_dense():
    score = _adaptive_min_score(500)
    assert 0.19 <= score <= 0.21


def test_adaptive_min_score_very_dense():
    score = _adaptive_min_score(5000)
    assert score <= 0.20  # Capped at lower bound


def test_adaptive_min_score_monotonic():
    """Threshold decreases monotonically as memory count increases."""
    prev = 1.0
    for count in [1, 5, 10, 50, 100, 499]:
        score = _adaptive_min_score(count)
        assert score < prev, f"count={count}: {score} >= {prev}"
        prev = score
    # At 500+ the function floors at 0.2
    assert _adaptive_min_score(500) == 0.2
    assert _adaptive_min_score(1000) == 0.2


# ── _apply_quality_gate ──


def test_quality_gate_empty():
    assert _apply_quality_gate([], 0.5, 0) == []


def test_quality_gate_filters_low_confidence():
    results = [
        {"id": 1, "_confidence_score": 0.8},
        {"id": 2, "_confidence_score": 0.2},
    ]
    filtered = _apply_quality_gate(results, 0.5, memory_count=30)
    assert len(filtered) == 1
    assert filtered[0]["id"] == 1


def test_quality_gate_filters_low_cosine():
    results = [
        {"id": 1, "_cosine": 0.6},
        {"id": 2, "_cosine": 0.1},
    ]
    filtered = _apply_quality_gate(results, 0.3, memory_count=30)
    assert len(filtered) == 1
    assert filtered[0]["id"] == 1


def test_quality_gate_rrf_only_uses_scaled_threshold():
    """v2.4.12: _rrf_score is compared against a scaled threshold (min_score * RRF_MAX_SCALE).

    At min_score=0.3 and default RRF_K=60 (so RRF_MAX_SCALE = 3/61 ≈ 0.0492),
    the RRF threshold is ≈ 0.3 * 0.0492 ≈ 0.0148.
    Rank-0 of a single retriever gives 1/(60+1) ≈ 0.0164 → passes.
    Rank-10 of a single retriever gives 1/71 ≈ 0.0141 → blocks.
    """
    results = [
        {"id": 1, "_rrf_score": 0.0164},
        {"id": 2, "_rrf_score": 0.0141},
    ]
    filtered = _apply_quality_gate(results, 0.3, memory_count=50)
    assert [r["id"] for r in filtered] == [1]


def test_quality_gate_rrf_multi_retriever_corroboration():
    """v2.4.12: Two retrievers at rank 0 (rrf ≈ 0.033) passes even in sparse pools."""
    results = [{"id": 1, "_rrf_score": 0.033}]  # 2 retrievers, rank 0 each
    # Sparse pool (count=5): min_score=_adaptive_min_score(5)≈0.41, rrf_thresh≈0.0202 → passes
    filtered = _apply_quality_gate(results, 0.41, memory_count=5)
    assert len(filtered) == 1


def test_quality_gate_cosine_takes_priority_over_rrf():
    """v2.4.12: When both _cosine and _rrf_score are present, _cosine drives the decision.

    Previously _rrf_score was selected first via falsy-chain, causing a
    high-cosine vector hit (e.g., 0.8) to be blocked by its tiny RRF score.
    """
    results = [{"id": 1, "_cosine": 0.8, "_rrf_score": 0.001}]
    filtered = _apply_quality_gate(results, 0.3, memory_count=50)
    assert len(filtered) == 1
    assert filtered[0]["id"] == 1


def test_quality_gate_low_cosine_blocks_despite_rrf():
    """v2.4.12: Low cosine blocks even when RRF would have passed on its own."""
    results = [{"id": 1, "_cosine": 0.1, "_rrf_score": 0.05}]
    assert _apply_quality_gate(results, 0.3, memory_count=50) == []


def test_quality_gate_confidence_takes_priority_over_cosine_and_rrf():
    """v2.4.12: confidence > cosine > rrf priority preserved (existing behavior)."""
    # confidence=0.2 blocks despite cosine=0.9 and a very high rrf
    results = [{"id": 1, "_confidence_score": 0.2, "_cosine": 0.9, "_rrf_score": 0.04}]
    assert _apply_quality_gate(results, 0.3, memory_count=50) == []


def test_quality_gate_rrf_mode_realistic_mixed_scenario():
    """v2.4.12 regression: realistic RRF-mode recall with mixed signals must
    return semantically-matched results instead of blocking everything.
    """
    results = [
        {"id": 1, "_cosine": 0.6, "_rrf_score": 0.033},  # vector+FTS hit, good cosine → PASS via cosine
        {"id": 2, "_cosine": 0.4, "_rrf_score": 0.016},  # vector only, decent cosine → PASS via cosine
        {"id": 3, "_rrf_score": 0.033},  # FTS-only, 2 retrievers rank 0 → PASS via scaled rrf
        {"id": 4, "_cosine": 0.1, "_rrf_score": 0.01},  # poor cosine → BLOCK via cosine
        {"id": 5, "_rrf_score": 0.005},  # FTS-only low rank → BLOCK via scaled rrf
    ]
    filtered = _apply_quality_gate(results, 0.3, memory_count=75)
    ids = [r["id"] for r in filtered]
    assert ids == [1, 2, 3]


def test_quality_gate_profile_blocked_sparse():
    """Profile is blocked when memory_count < 50."""
    results = [
        {"id": -1, "content": "[Profile] test", "source": {"System": "profile"}},
        {"id": 1, "_confidence_score": 0.8},
    ]
    filtered = _apply_quality_gate(results, 0.3, memory_count=30)
    assert len(filtered) == 1
    assert filtered[0]["id"] == 1


def test_quality_gate_profile_allowed_dense():
    """Profile is allowed when memory_count >= 50."""
    results = [
        {"id": -1, "content": "[Profile] test", "source": {"System": "profile"}},
    ]
    filtered = _apply_quality_gate(results, 0.3, memory_count=100)
    assert len(filtered) == 1
    assert filtered[0]["id"] == -1


def test_quality_gate_unscored_blocked_sparse():
    """Unscored results blocked when memory_count < 100."""
    results = [
        {"id": 1, "content": "keyword match only"},  # no _cosine, no _rrf_score
    ]
    filtered = _apply_quality_gate(results, 0.3, memory_count=50)
    assert len(filtered) == 0


def test_quality_gate_unscored_allowed_dense():
    """Unscored results allowed when memory_count >= 100."""
    results = [
        {"id": 1, "content": "keyword match only"},
    ]
    filtered = _apply_quality_gate(results, 0.3, memory_count=200)
    assert len(filtered) == 1


def test_quality_gate_mixed_results():
    """Mixed result types: scored, unscored, profile."""
    results = [
        {"id": 1, "_confidence_score": 0.8},  # passes
        {"id": 2, "_confidence_score": 0.1},  # fails
        {"id": 3, "content": "keyword only"},  # unscored, blocked (count=80 < 100)
        {"id": -1, "content": "[Profile]", "source": {"System": "profile"}},  # allowed (count=80 >= 50)
    ]
    filtered = _apply_quality_gate(results, 0.3, memory_count=80)
    assert len(filtered) == 2
    ids = [r["id"] for r in filtered]
    assert 1 in ids
    assert -1 in ids
