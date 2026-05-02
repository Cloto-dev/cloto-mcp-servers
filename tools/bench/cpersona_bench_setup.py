#!/usr/bin/env python3
"""
CPersona benchmark setup script.

Creates the dedicated benchmark agent (agent.cpersona_bench) in ClotoCore,
then loads the canonical 5-topic corpus into the CPersona DB with embeddings.

Usage:
    python3 cpersona_bench_setup.py [--reset] [--agent AGENT_ID]

Options:
    --reset     Delete existing benchmark memories before loading corpus
    --agent     Agent ID to use (default: agent.cpersona_bench)

Requirements:
    - ClotoCore running on port 8081
    - Embedding server running on port 8401
    - sqlite3 in PATH (for agent creation)
"""

import argparse
import json
import sqlite3
import struct
import sys
import urllib.request
import urllib.error
from datetime import datetime, timezone

# ── Configuration ──────────────────────────────────────────────────────────────

CLOTOCORE_API   = "http://127.0.0.1:8081/api"
EMBED_URL       = "http://127.0.0.1:8401/embed"
CLOTOCORE_DB    = "/Users/hachiya/Desktop/repos/ClotoCore/target/debug/data/cloto_memories.db"
CPERSONA_DB     = "/Users/hachiya/Desktop/repos/ClotoCore/dashboard/src-tauri/cpersona.db"
API_KEY         = "d6b705613200449d6c9e08ecf218b0571742937c9575c26982c5be29b10443f3"
DEFAULT_ENGINE  = "mind.deepseek"
BENCH_AGENT_ID  = "agent.cpersona_bench"

# ── Canonical 5-topic corpus ───────────────────────────────────────────────────

CORPUS: dict[str, list[str]] = {
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

# ── Helpers ────────────────────────────────────────────────────────────────────

def _embed_batch(texts: list[str], namespace: str) -> list[bytes]:
    """Embed a batch of texts and return as packed float32 blobs."""
    import json as _json
    data = _json.dumps({"texts": texts, "namespace": namespace}).encode()
    req = urllib.request.Request(
        EMBED_URL, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        embeddings = _json.loads(r.read())["embeddings"]
    return [struct.pack(f"{len(e)}f", *e) for e in embeddings]


def _check_services():
    print("Checking services...")
    try:
        req = urllib.request.Request(
            f"{CLOTOCORE_API}/system/health",
            headers={"X-API-Key": API_KEY})
        with urllib.request.urlopen(req, timeout=3) as r:
            print(f"  ClotoCore API: OK (port 8081)")
    except Exception as e:
        print(f"  ClotoCore API: FAILED — {e}")
        sys.exit(1)

    try:
        data = json.dumps({"texts": ["test"], "namespace": "bench"}).encode()
        req = urllib.request.Request(
            EMBED_URL, data=data, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as r:
            print(f"  Embedding server: OK (port 8401)")
    except Exception as e:
        print(f"  Embedding server: FAILED — {e}")
        sys.exit(1)


def _ensure_agent(agent_id: str):
    """Create agent in ClotoCore DB if it doesn't exist."""
    db = sqlite3.connect(CLOTOCORE_DB)
    try:
        row = db.execute("SELECT id FROM agents WHERE id=?", (agent_id,)).fetchone()
        if row:
            print(f"  Agent {agent_id}: already exists")
            return
        db.execute(
            "INSERT INTO agents (id, name, description, default_engine_id, status, metadata, required_capabilities, enabled, agent_type) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                agent_id,
                "CPersona Benchmark",
                "Dedicated agent for CPersona recall quality benchmarks. Do not use for production.",
                DEFAULT_ENGINE,
                "offline",
                json.dumps({"preferred_memory": "memory.cpersona"}),
                json.dumps([]),
                1,
                "agent",
            )
        )
        db.commit()
        print(f"  Agent {agent_id}: created")
    finally:
        db.close()


def _ensure_mcp_access(agent_id: str):
    """Grant memory.cpersona access to the benchmark agent."""
    db = sqlite3.connect(CLOTOCORE_DB)
    try:
        row = db.execute(
            "SELECT id FROM mcp_access_control WHERE agent_id=? AND server_id='memory.cpersona'",
            (agent_id,)
        ).fetchone()
        if row:
            print(f"  MCP access: already granted")
            return
        db.execute(
            "INSERT INTO mcp_access_control (agent_id, server_id, entry_type, permission, granted_at) VALUES (?, ?, ?, ?, ?)",
            (agent_id, "memory.cpersona", "server_grant", "allow", datetime.now(timezone.utc).isoformat())
        )
        db.commit()
        print(f"  MCP access: granted memory.cpersona")
    except Exception as e:
        print(f"  MCP access: skipped ({e})")
    finally:
        db.close()


def _get_schema_version() -> int:
    db = sqlite3.connect(CPERSONA_DB)
    try:
        row = db.execute("SELECT MAX(version) FROM schema_version").fetchone()
        return row[0] if row else 0
    finally:
        db.close()


def _reset_corpus(agent_id: str):
    """Delete ALL memories and episodes for the benchmark agent (full clean slate)."""
    db = sqlite3.connect(CPERSONA_DB)
    try:
        c1 = db.execute("DELETE FROM memories WHERE agent_id=?", (agent_id,))
        c2 = db.execute("DELETE FROM episodes WHERE agent_id=?", (agent_id,))
        db.commit()
        print(f"  Reset: deleted {c1.rowcount} memories, {c2.rowcount} episodes")
    finally:
        db.close()


def _load_corpus(agent_id: str):
    """Embed and store all corpus memories into CPersona DB."""
    db = sqlite3.connect(CPERSONA_DB)
    total = 0
    try:
        for topic, texts in CORPUS.items():
            print(f"  Embedding {topic} ({len(texts)} items)...", end=" ", flush=True)
            blobs = _embed_batch(texts, f"cpersona:{agent_id}")
            now = datetime.now(timezone.utc).isoformat()
            for text, blob in zip(texts, blobs):
                db.execute(
                    "INSERT INTO memories (agent_id, content, source, timestamp, embedding, channel, created_at, locked) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    (
                        agent_id,
                        text,
                        json.dumps({"System": "benchmark_corpus"}),
                        now,
                        blob,
                        f"bench_{topic}",
                        now,
                        0,
                    )
                )
                total += 1
            db.commit()
            print(f"done ({len(texts)} stored)")
    finally:
        db.close()
    return total


def _verify_corpus(agent_id: str):
    """Print corpus stats."""
    db = sqlite3.connect(CPERSONA_DB)
    try:
        rows = db.execute(
            "SELECT channel, COUNT(*) FROM memories WHERE agent_id=? GROUP BY channel ORDER BY channel",
            (agent_id,)
        ).fetchall()
        print(f"\n  Corpus in CPersona ({agent_id}):")
        for channel, cnt in rows:
            print(f"    {channel}: {cnt} memories")
        total = db.execute(
            "SELECT COUNT(*) FROM memories WHERE agent_id=?", (agent_id,)
        ).fetchone()[0]
        with_emb = db.execute(
            "SELECT COUNT(*) FROM memories WHERE agent_id=? AND embedding IS NOT NULL", (agent_id,)
        ).fetchone()[0]
        print(f"    Total: {total}  (with embeddings: {with_emb})")
    finally:
        db.close()


# ── Main ───────────────────────────────────────────────────────────────────────

def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--reset", action="store_true",
                   help="Delete existing benchmark memories before loading")
    p.add_argument("--agent", default=BENCH_AGENT_ID,
                   help=f"Agent ID (default: {BENCH_AGENT_ID})")
    args = p.parse_args()

    agent_id = args.agent
    print(f"\n{'='*55}")
    print(f"CPersona Benchmark Setup")
    print(f"Agent: {agent_id}")
    print(f"{'='*55}\n")

    _check_services()
    print()

    print("1. Setting up benchmark agent...")
    _ensure_agent(agent_id)
    _ensure_mcp_access(agent_id)
    print()

    print("2. Loading corpus into CPersona...")
    if args.reset:
        _reset_corpus(agent_id)
    _load_corpus(agent_id)
    print()

    print("3. Verification...")
    _verify_corpus(agent_id)

    print(f"\n{'='*55}")
    print(f"Setup complete. Run benchmark with:")
    print(f"  python3 cpersona_ab_runner.py --agent {agent_id}")
    print(f"{'='*55}\n")


if __name__ == "__main__":
    main()
