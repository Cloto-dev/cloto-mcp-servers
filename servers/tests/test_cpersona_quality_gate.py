"""Tests for CPersona v2.4.6 Adaptive Quality Gate."""

import sys

sys.path.insert(0, "cpersona")

import math
from datetime import datetime, timezone, timedelta

from server import _adaptive_min_score, _apply_quality_gate, _autocut, _build_context_query, _episode_boundary_factor  # noqa: E402, I001
from server import _get_vector_threshold, _agent_thresholds, VECTOR_MIN_SIMILARITY
from server import CONTEXT_QUERY_MAX_CHARS


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


# ── _autocut (v2.4.13 relative gap ratio) ──


def _r(cosine=None, rrf=None):
    """Helper: build a minimal result dict with the given score fields."""
    r = {"id": 1, "content": "x"}
    if cosine is not None:
        r["_cosine"] = cosine
    if rrf is not None:
        r["_rrf_score"] = rrf
    return r


def test_autocut_bread_vs_pi_case():
    """Canonical case: cosine 0.59 (bread) vs 0.31 (raspberry pi) — ratio 0.47 → cut."""
    results = [_r(cosine=0.59), _r(cosine=0.31)]
    assert len(_autocut(results)) == 1
    assert _autocut(results)[0]["_cosine"] == 0.59


def test_autocut_uniform_no_cut():
    """Evenly distributed scores — ratio 0.02 < 0.15 → no cut."""
    results = [_r(cosine=0.50), _r(cosine=0.49), _r(cosine=0.48)]
    assert _autocut(results) == results


def test_autocut_rrf_scale_large_gap():
    """RRF scale: [0.048, 0.030] — ratio 0.375 → cut to first."""
    results = [_r(rrf=0.048), _r(rrf=0.030)]
    assert len(_autocut(results)) == 1
    assert _autocut(results)[0]["_rrf_score"] == 0.048


def test_autocut_rrf_scale_no_cut():
    """RRF scale, uniform — ratio ~0.04 → no cut."""
    results = [_r(rrf=0.048), _r(rrf=0.046), _r(rrf=0.044)]
    assert _autocut(results) == results


def test_autocut_single_result():
    """Single result always returned unchanged."""
    results = [_r(cosine=0.80)]
    assert _autocut(results) == results


def test_autocut_empty():
    """Empty list returned unchanged."""
    assert _autocut([]) == []


def test_autocut_zero_max_score():
    """All-zero scores — no cut."""
    results = [_r(cosine=0.0), _r(cosine=0.0)]
    assert _autocut(results) == results


def test_autocut_gap_ratio_below_threshold():
    """Gap ratio exactly at boundary (< 0.15) → no cut."""
    # gap=0.05, max=0.50, ratio=0.10 < 0.15
    results = [_r(cosine=0.50), _r(cosine=0.45)]
    assert _autocut(results) == results


def test_autocut_uses_cosine_when_no_rrf():
    """Falls back to _cosine when _rrf_score is absent."""
    results = [_r(cosine=0.70), _r(cosine=0.30)]
    cut = _autocut(results)
    assert len(cut) == 1
    assert cut[0]["_cosine"] == 0.70


def test_autocut_uses_rrf_when_present():
    """Uses _rrf_score (takes priority via `or` chain) when present."""
    results = [_r(rrf=0.045, cosine=0.90), _r(rrf=0.020, cosine=0.85)]
    # rrf scores: [0.045, 0.020], ratio = 0.025/0.045 = 0.56 → cut
    cut = _autocut(results)
    assert len(cut) == 1
    assert cut[0]["_rrf_score"] == 0.045


def test_autocut_three_results_cut_at_largest_gap():
    """Three results: [0.80, 0.78, 0.40] — largest gap after index 1 → keep first two."""
    results = [_r(cosine=0.80), _r(cosine=0.78), _r(cosine=0.40)]
    cut = _autocut(results)
    assert len(cut) == 2
    assert cut[0]["_cosine"] == 0.80
    assert cut[1]["_cosine"] == 0.78


# ── _episode_boundary_factor (v2.4.14) ──

_NOW = datetime(2026, 4, 24, 12, 0, 0, tzinfo=timezone.utc)


def _boundary(hours_ago: float) -> datetime:
    return _NOW - timedelta(hours=hours_ago)


def _mem_ts(hours_before_boundary: float, boundary: datetime = _NOW) -> str:
    dt = boundary - timedelta(hours=hours_before_boundary)
    return dt.isoformat()


def test_episode_penalty_current_session_no_decay():
    """Memory after the boundary (current session) → factor=1.0."""
    boundary = _NOW
    mem_ts = (boundary + timedelta(hours=1)).isoformat()
    assert _episode_boundary_factor(mem_ts, boundary) == 1.0


def test_episode_penalty_at_boundary_no_decay():
    """Memory exactly at the boundary → factor=1.0 (>= boundary passes)."""
    assert _episode_boundary_factor(_NOW.isoformat(), _NOW) == 1.0


def test_episode_penalty_24h_before():
    """24h before boundary → exp(-0.01 × 24) ≈ 0.787."""
    factor = _episode_boundary_factor(_mem_ts(24), _NOW)
    assert abs(factor - math.exp(-0.01 * 24)) < 1e-6


def test_episode_penalty_floor_clamp():
    """Very old memory (1000h) → clamped at EPISODE_DECAY_FLOOR=0.5."""
    factor = _episode_boundary_factor(_mem_ts(1000), _NOW)
    assert factor == 0.5  # default EPISODE_DECAY_FLOOR


def test_episode_penalty_no_boundary():
    """episode_boundary_ts=None → factor=1.0 (penalty disabled)."""
    assert _episode_boundary_factor("2026-04-01T00:00:00+00:00", None) == 1.0


def test_episode_penalty_no_timestamp():
    """memory_ts_str=None → factor=1.0 (missing timestamp)."""
    assert _episode_boundary_factor(None, _NOW) == 1.0


def test_episode_penalty_empty_timestamp():
    """Empty timestamp string → factor=1.0."""
    assert _episode_boundary_factor("", _NOW) == 1.0


def test_episode_penalty_raspi_case():
    """Canonical case: raspberry pi memory 24h before boundary.

    Original cosine 0.327, threshold 0.271.
    Penalised cosine 0.327 × 0.787 = 0.257 < 0.271 → would be filtered.
    """
    factor = _episode_boundary_factor(_mem_ts(24), _NOW)
    penalised = 0.327 * factor
    assert penalised < 0.271  # below quality gate threshold


# ── _build_context_query (v2.4.15 CQB) ──


def test_build_context_query_basic():
    """Context hint + query → newline-separated combination."""
    result = _build_context_query("この前のパンの話覚えてる?", "パン屋さん 朝食")
    assert result == "パン屋さん 朝食\nこの前のパンの話覚えてる?"


def test_build_context_query_empty_hint():
    """Empty or whitespace-only hint → original query unchanged."""
    assert _build_context_query("クエリ", "") == "クエリ"
    assert _build_context_query("クエリ", "   ") == "クエリ"


def test_build_context_query_truncation():
    """Hint longer than MAX_CHARS is truncated from the tail."""
    long_hint = "a" * (CONTEXT_QUERY_MAX_CHARS + 50)
    result = _build_context_query("q", long_hint)
    # Tail (most recent content) should survive
    tail = "a" * CONTEXT_QUERY_MAX_CHARS
    assert result == f"{tail}\nq"
    assert len(result.split("\n")[0]) == CONTEXT_QUERY_MAX_CHARS


def test_build_context_query_exact_max_chars():
    """Hint exactly MAX_CHARS long is not truncated."""
    exact_hint = "x" * CONTEXT_QUERY_MAX_CHARS
    result = _build_context_query("q", exact_hint)
    assert result == f"{exact_hint}\nq"


# ── _get_vector_threshold (per-agent dict) ──


def test_get_vector_threshold_falls_back_to_global():
    """Agent not in dict → global VECTOR_MIN_SIMILARITY returned."""
    _agent_thresholds.pop("__test_agent_fallback__", None)
    assert _get_vector_threshold("__test_agent_fallback__") == VECTOR_MIN_SIMILARITY


def test_get_vector_threshold_returns_per_agent_value():
    """Agent in dict → dict value returned, not global."""
    _agent_thresholds["__test_agent_custom__"] = 0.1234
    try:
        assert _get_vector_threshold("__test_agent_custom__") == 0.1234
        assert _get_vector_threshold("__test_agent_custom__") != VECTOR_MIN_SIMILARITY
    finally:
        _agent_thresholds.pop("__test_agent_custom__", None)


def test_get_vector_threshold_empty_agent_id_falls_back_to_global():
    """Empty string is not a valid agent key — falls back to global."""
    _agent_thresholds.pop("", None)
    assert _get_vector_threshold("") == VECTOR_MIN_SIMILARITY


def test_get_vector_threshold_independent_per_agent():
    """Two agents can hold different thresholds independently."""
    _agent_thresholds["__test_a__"] = 0.20
    _agent_thresholds["__test_b__"] = 0.35
    try:
        assert _get_vector_threshold("__test_a__") == 0.20
        assert _get_vector_threshold("__test_b__") == 0.35
        assert _get_vector_threshold("__test_c__") == VECTOR_MIN_SIMILARITY
    finally:
        _agent_thresholds.pop("__test_a__", None)
        _agent_thresholds.pop("__test_b__", None)
