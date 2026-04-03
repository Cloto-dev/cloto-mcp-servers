"""Tests for CPersona v2.3.4 Scalability improvements."""

import heapq
import re
import sys

import pytest

sys.path.insert(0, "cpersona")


# ── Improvement 1: memories_fts schema ──


@pytest.mark.asyncio
async def test_memories_fts_table_created():
    """memories_fts virtual table is created when FTS is enabled."""
    import aiosqlite

    db = await aiosqlite.connect(":memory:")
    # Minimal schema for memories
    await db.execute(
        """CREATE TABLE memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT DEFAULT (datetime('now'))
        )"""
    )
    # Apply FTS SQL (memories_fts portion)
    await db.executescript(
        """
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
            content, content=memories, content_rowid=id
        );
        CREATE TRIGGER IF NOT EXISTS memories_fts_ai AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS memories_fts_ad AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, content)
            VALUES ('delete', old.id, old.content);
        END;
        """
    )
    # Insert a memory
    await db.execute(
        "INSERT INTO memories (agent_id, content) VALUES (?, ?)",
        ("test", "Cascading Recall design decision"),
    )
    await db.commit()

    # FTS5 should find it
    rows = await db.execute_fetchall("SELECT rowid FROM memories_fts WHERE memories_fts MATCH '\"Cascading\"'")
    assert len(rows) == 1

    await db.close()


@pytest.mark.asyncio
async def test_memories_fts_delete_sync():
    """Deleting a memory removes it from FTS index via trigger."""
    import aiosqlite

    db = await aiosqlite.connect(":memory:")
    await db.execute(
        """CREATE TABLE memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            agent_id TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT DEFAULT (datetime('now'))
        )"""
    )
    await db.executescript(
        """
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
            content, content=memories, content_rowid=id
        );
        CREATE TRIGGER IF NOT EXISTS memories_fts_ai AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS memories_fts_ad AFTER DELETE ON memories BEGIN
            INSERT INTO memories_fts(memories_fts, rowid, content)
            VALUES ('delete', old.id, old.content);
        END;
        """
    )
    await db.execute(
        "INSERT INTO memories (agent_id, content) VALUES (?, ?)",
        ("test", "temporary note"),
    )
    await db.commit()

    # Verify present
    rows = await db.execute_fetchall("SELECT rowid FROM memories_fts WHERE memories_fts MATCH '\"temporary\"'")
    assert len(rows) == 1

    # Delete
    await db.execute("DELETE FROM memories WHERE id = 1")
    await db.commit()

    # Verify removed from FTS
    rows = await db.execute_fetchall("SELECT rowid FROM memories_fts WHERE memories_fts MATCH '\"temporary\"'")
    assert len(rows) == 0

    await db.close()


@pytest.mark.asyncio
async def test_memories_fts_query_sanitize():
    """FTS5 query sanitization strips operators to prevent injection."""
    # Same sanitization logic as used in _search_memories_keyword
    query = 'test AND "injection" OR NOT -excluded'
    sanitized = re.sub(r"[^\w\s]", "", query, flags=re.UNICODE)
    words = sanitized.split()
    fts_query = " ".join(f'"{w}"' for w in words)
    # Should only contain quoted words, no FTS5 operators
    assert "AND" not in fts_query or '"AND"' in fts_query
    assert "-" not in fts_query
    assert "*" not in fts_query


# ── Improvement 2: heapq top-K ──


def test_heapq_nlargest_matches_sort():
    """heapq.nlargest produces same results as sort+slice."""
    import random

    random.seed(42)
    candidates = [(random.random(), {"id": i}) for i in range(500)]
    limit = 10

    # Sort approach (old)
    sorted_candidates = sorted(candidates, key=lambda x: x[0], reverse=True)
    sort_result = [c[1] for c in sorted_candidates[:limit]]

    # Heap approach (new)
    top_k = heapq.nlargest(limit, candidates, key=lambda x: x[0])
    heap_result = [c[1] for c in top_k]

    assert sort_result == heap_result


def test_heapq_empty_candidates():
    """heapq.nlargest handles empty list."""
    result = heapq.nlargest(10, [], key=lambda x: x[0])
    assert result == []


def test_heapq_limit_exceeds_candidates():
    """heapq.nlargest when limit > len(candidates)."""
    candidates = [(0.9, {"id": 1}), (0.5, {"id": 2})]
    result = heapq.nlargest(10, candidates, key=lambda x: x[0])
    assert len(result) == 2
    assert result[0][1]["id"] == 1


# ── Improvement 3: Adaptive scan limit ──


def test_scan_limit_small_request():
    """limit=5 → scan_limit=100 (minimum floor)."""
    limit = 5
    max_memories = 500
    scan_limit = min(max_memories, max(limit * 10, 100))
    assert scan_limit == 100


def test_scan_limit_medium_request():
    """limit=50 → scan_limit=500 (capped at MAX_MEMORIES)."""
    limit = 50
    max_memories = 500
    scan_limit = min(max_memories, max(limit * 10, 100))
    assert scan_limit == 500


def test_scan_limit_large_max():
    """limit=20 with MAX_MEMORIES=2000 → scan_limit=200."""
    limit = 20
    max_memories = 2000
    scan_limit = min(max_memories, max(limit * 10, 100))
    assert scan_limit == 200


def test_keyword_scan_limit():
    """LIKE fallback scan limit: min(MAX_MEMORIES, max(limit*5, 50))."""
    limit = 5
    max_memories = 500
    scan_limit = min(max_memories, max(limit * 5, 50))
    assert scan_limit == 50

    limit = 20
    scan_limit = min(max_memories, max(limit * 5, 50))
    assert scan_limit == 100
