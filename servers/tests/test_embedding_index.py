"""Tests for embedding server v0.2.0 Vector Index."""

import os
import struct
import sys

import numpy as np
import pytest

# Ensure embedding/server.py is found before cpersona/server.py
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "embedding"))


# ── VectorIndex Unit Tests ──


@pytest.mark.asyncio
async def test_vector_index_lifecycle():
    """Index → search → remove lifecycle."""
    import aiosqlite

    # Minimal mock provider
    class MockProvider:
        async def embed(self, texts):
            # Return deterministic unit vectors based on text hash
            results = []
            for text in texts:
                vec = np.random.default_rng(hash(text) % 2**31).random(384).astype(np.float32)
                vec = vec / np.linalg.norm(vec)
                results.append(vec.tolist())
            return results

        def dimensions(self):
            return 384

    import importlib.util

    _spec = importlib.util.spec_from_file_location(
        "embedding_server",
        os.path.join(os.path.dirname(__file__), "..", "embedding", "server.py"),
    )
    _mod = importlib.util.module_from_spec(_spec)
    _spec.loader.exec_module(_mod)
    VectorIndex = _mod.VectorIndex

    idx = VectorIndex(":memory:")
    # Manually init with in-memory DB
    idx._db = await aiosqlite.connect(":memory:")
    await idx._db.execute("PRAGMA journal_mode=WAL")
    await idx._db.executescript(
        """
        CREATE TABLE vectors (
            namespace TEXT NOT NULL,
            item_id   TEXT NOT NULL,
            vector    BLOB NOT NULL,
            created_at TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (namespace, item_id)
        );
        """
    )
    await idx._db.commit()
    idx._index = {}

    provider = MockProvider()

    # Index
    count = await idx.index("test:ns", [{"id": "a", "text": "hello"}, {"id": "b", "text": "world"}], provider)
    assert count == 2
    assert await idx.count("test:ns") == 2

    # Search
    results = await idx.search("test:ns", "hello", 10, 0.0, provider)
    assert len(results) > 0
    assert results[0]["id"] in ("a", "b")
    assert "score" in results[0]

    # Remove
    removed = await idx.remove("test:ns", ["a"])
    assert removed == 1
    assert await idx.count("test:ns") == 1

    # Search after remove — only "b" remains
    results = await idx.search("test:ns", "hello", 10, 0.0, provider)
    assert all(r["id"] != "a" for r in results)

    await idx._db.close()


@pytest.mark.asyncio
async def test_vector_index_namespace_isolation():
    """Different namespaces are isolated."""
    import aiosqlite

    class MockProvider:
        async def embed(self, texts):
            results = []
            for text in texts:
                vec = np.random.default_rng(hash(text) % 2**31).random(384).astype(np.float32)
                vec = vec / np.linalg.norm(vec)
                results.append(vec.tolist())
            return results

        def dimensions(self):
            return 384

    import importlib.util

    _spec = importlib.util.spec_from_file_location(
        "embedding_server",
        os.path.join(os.path.dirname(__file__), "..", "embedding", "server.py"),
    )
    _mod = importlib.util.module_from_spec(_spec)
    _spec.loader.exec_module(_mod)
    VectorIndex = _mod.VectorIndex

    idx = VectorIndex(":memory:")
    idx._db = await aiosqlite.connect(":memory:")
    await idx._db.executescript(
        """
        CREATE TABLE vectors (
            namespace TEXT NOT NULL,
            item_id   TEXT NOT NULL,
            vector    BLOB NOT NULL,
            created_at TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (namespace, item_id)
        );
        """
    )
    await idx._db.commit()
    idx._index = {}

    provider = MockProvider()

    await idx.index("cpersona:alice", [{"id": "m1", "text": "alice memory"}], provider)
    await idx.index("cpersona:bob", [{"id": "m1", "text": "bob memory"}], provider)

    assert await idx.count("cpersona:alice") == 1
    assert await idx.count("cpersona:bob") == 1

    # Remove from alice doesn't affect bob
    await idx.remove("cpersona:alice", ["m1"])
    assert await idx.count("cpersona:alice") == 0
    assert await idx.count("cpersona:bob") == 1

    await idx._db.close()


@pytest.mark.asyncio
async def test_vector_index_upsert():
    """Indexing same ID twice replaces the vector."""
    import aiosqlite

    call_count = 0

    class MockProvider:
        async def embed(self, texts):
            nonlocal call_count
            call_count += 1
            results = []
            for text in texts:
                vec = np.random.default_rng(hash(text) % 2**31 + call_count).random(384).astype(np.float32)
                vec = vec / np.linalg.norm(vec)
                results.append(vec.tolist())
            return results

        def dimensions(self):
            return 384

    import importlib.util

    _spec = importlib.util.spec_from_file_location(
        "embedding_server",
        os.path.join(os.path.dirname(__file__), "..", "embedding", "server.py"),
    )
    _mod = importlib.util.module_from_spec(_spec)
    _spec.loader.exec_module(_mod)
    VectorIndex = _mod.VectorIndex

    idx = VectorIndex(":memory:")
    idx._db = await aiosqlite.connect(":memory:")
    await idx._db.executescript(
        """
        CREATE TABLE vectors (
            namespace TEXT NOT NULL,
            item_id   TEXT NOT NULL,
            vector    BLOB NOT NULL,
            created_at TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (namespace, item_id)
        );
        """
    )
    await idx._db.commit()
    idx._index = {}

    provider = MockProvider()

    await idx.index("ns", [{"id": "x", "text": "first"}], provider)
    vec1 = idx._index["ns"]["x"].copy()

    await idx.index("ns", [{"id": "x", "text": "second"}], provider)
    vec2 = idx._index["ns"]["x"]

    # Vector should be different (different text + call_count)
    assert not np.allclose(vec1, vec2)
    # But count should still be 1
    assert await idx.count("ns") == 1

    await idx._db.close()


def test_blob_pack_unpack_roundtrip():
    """float32 pack/unpack is lossless."""
    original = np.random.default_rng(42).random(384).astype(np.float32)
    blob = struct.pack(f"<{len(original)}f", *original)
    restored = np.frombuffer(blob, dtype=np.float32)
    np.testing.assert_array_equal(original, restored)


@pytest.mark.asyncio
async def test_vector_index_empty_search():
    """Search on empty namespace returns empty list."""
    import aiosqlite

    class MockProvider:
        async def embed(self, texts):
            return [np.zeros(384).tolist() for _ in texts]

        def dimensions(self):
            return 384

    import importlib.util

    _spec = importlib.util.spec_from_file_location(
        "embedding_server",
        os.path.join(os.path.dirname(__file__), "..", "embedding", "server.py"),
    )
    _mod = importlib.util.module_from_spec(_spec)
    _spec.loader.exec_module(_mod)
    VectorIndex = _mod.VectorIndex

    idx = VectorIndex(":memory:")
    idx._db = await aiosqlite.connect(":memory:")
    idx._index = {}

    results = await idx.search("nonexistent", "query", 10, 0.0, MockProvider())
    assert results == []

    await idx._db.close()
