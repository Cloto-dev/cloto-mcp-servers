"""Tests for CPersona Memory Confidence Score (v2.3.2+, updated for v2.4.4 DECAY_FLOOR=0.3)."""

import sys
from datetime import datetime, timedelta, timezone
from unittest.mock import patch

sys.path.insert(0, "cpersona")
from server import _compute_confidence  # noqa: E402

# ── Score Computation Tests ──


def test_confidence_high_cos_new():
    """High relevance + just now → score ≈ 0.90."""
    now_iso = datetime.now(timezone.utc).isoformat()
    result = _compute_confidence(0.65, now_iso)
    assert result["cosine"] == 0.65
    assert result["age_hours"] < 0.1
    assert 0.85 <= result["score"] <= 0.95


def test_confidence_high_cos_old():
    """High relevance + 1 week old → score ≈ 0.59."""
    one_week_ago = (datetime.now(timezone.utc) - timedelta(hours=168)).isoformat()
    result = _compute_confidence(0.55, one_week_ago)
    assert 0.50 <= result["score"] <= 0.65


def test_confidence_high_cos_very_old():
    """High relevance + 6 months → score drops but DECAY_FLOOR=0.3 prevents extinction."""
    six_months_ago = (datetime.now(timezone.utc) - timedelta(hours=4380)).isoformat()
    result = _compute_confidence(0.60, six_months_ago)
    # v2.4.4: DECAY_FLOOR raised to 0.3 — old memories decay to floor, not zero
    # time_decay = max(0.3, 1/(1+4380*0.005)) = 0.3
    # norm_cos = (0.60-0.20)/(0.75-0.20) ≈ 0.727
    # score = sqrt(0.727 * 0.3) ≈ 0.467
    assert 0.40 <= result["score"] <= 0.55


def test_confidence_low_cos_new():
    """Low relevance + just now → score ≈ 0.30."""
    now_iso = datetime.now(timezone.utc).isoformat()
    result = _compute_confidence(0.25, now_iso)
    assert 0.20 <= result["score"] <= 0.40


def test_confidence_low_cos_old():
    """Low relevance + 6 months → low score, floored by DECAY_FLOOR=0.3."""
    six_months_ago = (datetime.now(timezone.utc) - timedelta(hours=4380)).isoformat()
    result = _compute_confidence(0.25, six_months_ago)
    # v2.4.4: DECAY_FLOOR=0.3 prevents full extinction
    # norm_cos = (0.25-0.20)/(0.75-0.20) ≈ 0.091
    # score = sqrt(0.091 * 0.3) ≈ 0.165
    assert 0.10 <= result["score"] <= 0.25


def test_confidence_no_cosine():
    """Non-vector results use sqrt(time_decay) only."""
    one_hour_ago = (datetime.now(timezone.utc) - timedelta(hours=1)).isoformat()
    result = _compute_confidence(None, one_hour_ago)
    assert "cosine" not in result
    assert result["score"] > 0.99  # time_decay ≈ 0.995, sqrt ≈ 0.997


def test_confidence_no_cosine_old():
    """Non-vector result from 1 week ago."""
    one_week_ago = (datetime.now(timezone.utc) - timedelta(hours=168)).isoformat()
    result = _compute_confidence(None, one_week_ago)
    assert "cosine" not in result
    assert 0.70 <= result["score"] <= 0.80


def test_confidence_null_timestamp():
    """Empty timestamp → age_hours = 0."""
    result = _compute_confidence(0.50, "")
    assert result["age_hours"] == 0.0
    # time_decay = 1.0, norm_cos = (0.50-0.20)/(0.75-0.20) ≈ 0.545
    # score = sqrt(0.545 * 1.0) ≈ 0.738
    assert 0.70 <= result["score"] <= 0.80


def test_confidence_null_timestamp_no_cosine():
    """Empty timestamp + no cosine → score = 1.0."""
    result = _compute_confidence(None, "")
    assert result["age_hours"] == 0.0
    assert result["score"] == 1.0


# ── Cosine Floor/Ceil Clamping ──


@patch("server.COSINE_FLOOR", 0.20)
@patch("server.COSINE_CEIL", 0.75)
def test_cosine_below_floor():
    """Cosine below floor → norm_cos = 0.0 → score = 0.0."""
    now_iso = datetime.now(timezone.utc).isoformat()
    result = _compute_confidence(0.10, now_iso)
    assert result["cosine"] == 0.10
    assert result["score"] == 0.0


@patch("server.COSINE_FLOOR", 0.20)
@patch("server.COSINE_CEIL", 0.75)
def test_cosine_above_ceil():
    """Cosine above ceil → norm_cos = 1.0."""
    now_iso = datetime.now(timezone.utc).isoformat()
    result = _compute_confidence(0.90, now_iso)
    assert result["cosine"] == 0.90
    # norm_cos = 1.0, time_decay ≈ 1.0, score ≈ 1.0
    assert result["score"] >= 0.99


# ── Output Structure ──


def test_confidence_fields_with_cosine():
    """Vector result has cosine, age_hours, score."""
    now_iso = datetime.now(timezone.utc).isoformat()
    result = _compute_confidence(0.50, now_iso)
    assert "cosine" in result
    assert "age_hours" in result
    assert "score" in result
    assert isinstance(result["cosine"], float)
    assert isinstance(result["age_hours"], float)
    assert isinstance(result["score"], float)


def test_confidence_fields_without_cosine():
    """Non-vector result has age_hours, score but not cosine."""
    now_iso = datetime.now(timezone.utc).isoformat()
    result = _compute_confidence(None, now_iso)
    assert "cosine" not in result
    assert "age_hours" in result
    assert "score" in result


def test_score_range():
    """Score is always in [0.0, 1.0]."""
    cases = [
        (0.0, ""),
        (1.0, ""),
        (0.50, datetime.now(timezone.utc).isoformat()),
        (0.50, (datetime.now(timezone.utc) - timedelta(hours=10000)).isoformat()),
        (None, ""),
        (None, (datetime.now(timezone.utc) - timedelta(hours=10000)).isoformat()),
    ]
    for cos, ts in cases:
        result = _compute_confidence(cos, ts)
        assert 0.0 <= result["score"] <= 1.0, f"Score out of range for cos={cos}, ts={ts}"
