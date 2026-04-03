"""Tests for CPersona v2.4.6 Adaptive Quality Gate."""

import sys

sys.path.insert(0, "cpersona")
from server import _adaptive_min_score, _apply_quality_gate  # noqa: E402


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


def test_quality_gate_filters_low_rrf():
    results = [
        {"id": 1, "_rrf_score": 0.05},
        {"id": 2, "_rrf_score": 0.001},
    ]
    filtered = _apply_quality_gate(results, 0.01, memory_count=30)
    assert len(filtered) == 1
    assert filtered[0]["id"] == 1


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
        {"id": 1, "_confidence_score": 0.8},   # passes
        {"id": 2, "_confidence_score": 0.1},   # fails
        {"id": 3, "content": "keyword only"},   # unscored, blocked (count=80 < 100)
        {"id": -1, "content": "[Profile]", "source": {"System": "profile"}},  # allowed (count=80 >= 50)
    ]
    filtered = _apply_quality_gate(results, 0.3, memory_count=80)
    assert len(filtered) == 2
    ids = [r["id"] for r in filtered]
    assert 1 in ids
    assert -1 in ids
