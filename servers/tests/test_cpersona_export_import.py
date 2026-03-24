"""Tests for CPersona export_memories / import_memories tools."""

import asyncio
import json
import os
import tempfile

import pytest

# Patch DB path before importing server module
_tmpdir = tempfile.mkdtemp()
_test_db = os.path.join(_tmpdir, "test_export.db")
os.environ["CPERSONA_DB_PATH"] = _test_db
os.environ["CPERSONA_EMBEDDING_MODE"] = "none"
os.environ["CPERSONA_TASK_QUEUE_ENABLED"] = "false"

from cpersona.server import (
    close_db,
    do_export_memories,
    do_import_memories,
    do_store,
    get_db,
)


@pytest.fixture(autouse=True)
async def _setup_db():
    """Ensure fresh DB for each test."""
    global _test_db
    # Reset the module-level _db so get_db() creates a fresh connection
    import cpersona.server as mod

    mod._db = None
    # Use a unique DB per test to avoid state leakage
    _test_db = os.path.join(_tmpdir, f"test_{id(asyncio.get_event_loop())}.db")
    mod.DB_PATH = _test_db
    os.environ["CPERSONA_DB_PATH"] = _test_db
    yield
    await close_db()
    if os.path.exists(_test_db):
        os.remove(_test_db)


async def _seed_data(agent_id: str = "agent.test") -> dict:
    """Insert test memories and return counts."""
    for i in range(3):
        await do_store(
            agent_id,
            {
                "id": f"msg-{i}",
                "content": f"Test memory content {i}",
                "source": {"User": "tester"},
                "timestamp": f"2026-03-24T10:0{i}:00Z",
            },
        )
    return {"memories": 3}


@pytest.mark.asyncio
async def test_export_creates_valid_jsonl():
    """Export should produce valid JSONL with header + memory records."""
    await _seed_data()
    out_path = os.path.join(_tmpdir, "export_test.jsonl")

    result = await do_export_memories("agent.test", out_path)

    assert result["ok"] is True
    assert result["memories"] == 3
    assert os.path.exists(out_path)

    with open(out_path, encoding="utf-8") as f:
        lines = [json.loads(line) for line in f if line.strip()]

    # Header + 3 memories
    assert len(lines) == 4
    assert lines[0]["_type"] == "header"
    assert lines[0]["version"] == "cpersona-export/1.0"
    assert lines[0]["memory_count"] == 3
    for i, line in enumerate(lines[1:], 0):
        assert line["_type"] == "memory"
        assert line["agent_id"] == "agent.test"
        assert f"Test memory content" in line["content"]

    os.remove(out_path)


@pytest.mark.asyncio
async def test_export_all_agents():
    """Export with empty agent_id should include all agents."""
    await _seed_data("agent.alpha")
    await _seed_data("agent.beta")
    out_path = os.path.join(_tmpdir, "export_all.jsonl")

    result = await do_export_memories("", out_path)

    assert result["ok"] is True
    assert result["memories"] == 6

    os.remove(out_path)


@pytest.mark.asyncio
async def test_import_roundtrip():
    """Export → Import into fresh DB should preserve data."""
    await _seed_data("agent.test")
    export_path = os.path.join(_tmpdir, "roundtrip.jsonl")

    # Export
    await do_export_memories("agent.test", export_path)

    # Clear DB
    db = await get_db()
    await db.execute("DELETE FROM memories")
    await db.commit()

    # Import
    result = await do_import_memories(export_path)

    assert result["ok"] is True
    assert result["imported_memories"] == 3
    assert result["skipped_memories"] == 0

    # Verify data exists
    rows = await db.execute_fetchall("SELECT COUNT(*) FROM memories WHERE agent_id = 'agent.test'")
    assert rows[0][0] == 3

    os.remove(export_path)


@pytest.mark.asyncio
async def test_import_deduplication():
    """Importing the same file twice should skip duplicates (msg_id dedup)."""
    await _seed_data("agent.test")
    export_path = os.path.join(_tmpdir, "dedup.jsonl")
    await do_export_memories("agent.test", export_path)

    # First import (memories already exist with same msg_id)
    result = await do_import_memories(export_path)

    assert result["ok"] is True
    assert result["imported_memories"] == 0
    assert result["skipped_memories"] == 3

    os.remove(export_path)


@pytest.mark.asyncio
async def test_import_agent_remap():
    """Import with target_agent_id should remap all records."""
    await _seed_data("agent.source")
    export_path = os.path.join(_tmpdir, "remap.jsonl")
    await do_export_memories("agent.source", export_path)

    # Import with remap to different agent
    result = await do_import_memories(export_path, target_agent_id="agent.target")

    assert result["ok"] is True
    assert result["imported_memories"] == 3

    # Verify remapped
    db = await get_db()
    rows = await db.execute_fetchall("SELECT COUNT(*) FROM memories WHERE agent_id = 'agent.target'")
    assert rows[0][0] == 3

    os.remove(export_path)


@pytest.mark.asyncio
async def test_import_dry_run():
    """Dry run should count records without writing."""
    await _seed_data("agent.test")
    export_path = os.path.join(_tmpdir, "dryrun.jsonl")
    await do_export_memories("agent.test", export_path)

    # Clear DB
    db = await get_db()
    await db.execute("DELETE FROM memories")
    await db.commit()

    # Dry run
    result = await do_import_memories(export_path, dry_run=True)

    assert result["ok"] is True
    assert result["dry_run"] is True
    assert result["imported_memories"] == 3

    # Verify nothing written
    rows = await db.execute_fetchall("SELECT COUNT(*) FROM memories")
    assert rows[0][0] == 0

    os.remove(export_path)


@pytest.mark.asyncio
async def test_import_missing_file():
    """Import should return error for missing file."""
    result = await do_import_memories("/nonexistent/path.jsonl")
    assert "error" in result
