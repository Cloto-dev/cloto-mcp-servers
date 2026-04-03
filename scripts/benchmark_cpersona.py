"""CPersona Recall Benchmark via ClotoCore API.

Stores test memories, then measures recall accuracy with known queries.
Usage: python scripts/benchmark_cpersona.py
"""

import json
import time
import httpx

API_BASE = "http://127.0.0.1:8081"
API_KEY = "d6b705613200449d6c9e08ecf218b0571742937c9575c26982c5be29b10443f3"
SERVER_ID = "memory.cpersona"
AGENT_ID = "benchmark-test"

HEADERS = {"X-API-Key": API_KEY, "Content-Type": "application/json"}


def call_tool(client: httpx.Client, tool_name: str, arguments: dict, retries: int = 3) -> dict:
    for attempt in range(retries):
        resp = client.post(
            f"{API_BASE}/api/mcp/call",
            headers=HEADERS,
            json={"server_id": SERVER_ID, "tool_name": tool_name, "arguments": arguments},
        )
        if resp.status_code == 429:
            time.sleep(2 * (attempt + 1))
            continue
        resp.raise_for_status()
        return resp.json()
    resp.raise_for_status()
    return {}


# --- Test Data ---

MEMORIES = [
    {"id": "m01", "content": "ユーザーはRustとPythonを主に使うバックエンドエンジニアである"},
    {"id": "m02", "content": "プロジェクトClotoCoreはTauriベースのデスクトップアプリケーションである"},
    {"id": "m03", "content": "CPersonaはSQLiteとFTS5を使った永続記憶システムである"},
    {"id": "m04", "content": "MCPプロトコルはAnthropicが2024年11月に公開した"},
    {"id": "m05", "content": "Discord botのメンション限定フィルタを実装した"},
    {"id": "m06", "content": "Confidence Scoreは幾何平均で計算される: sqrt(norm_cos * time_decay)"},
    {"id": "m07", "content": "embedding serverにVectorIndex機能を追加してv0.2.0にした"},
    {"id": "m08", "content": "DECAY_RATEは0.005で半減期は約200時間（8日）である"},
    {"id": "m09", "content": "all-MiniLM-L6-v2は384次元のベクトルを出力するONNXモデルである"},
    {"id": "m10", "content": "Cascading Recallは4段階: ベクトル→FTS5→プロファイル→キーワード"},
    {"id": "m11", "content": "Anti-Contamination はagent_idによる19箇所のSQLフィルタリングで実現"},
    {"id": "m12", "content": "archive_episodeのpre-computedモードはLLM呼び出しをスキップする"},
    {"id": "m13", "content": "update_profileはLLMでファクトを抽出し既存プロファイルとマージする"},
    {"id": "m14", "content": "FTS5の外部コンテンツモードはインデックスのみ保持しテキストは元テーブル参照"},
    {"id": "m15", "content": "heapq.nlargestはO(N log K)でsort+sliceのO(N log N)より効率的"},
    {"id": "m16", "content": "memories_ftsはv2.3.4で追加されたFTS5インデックスである"},
    {"id": "m17", "content": "remote vector searchモードではembedding serverの/searchに委譲する"},
    {"id": "m18", "content": "CPERSONA_STORE_BLOBをfalseにするとDBサイズを削減できる"},
    {"id": "m19", "content": "タスクキューはpending_memory_tasksテーブルに永続化される"},
    {"id": "m20", "content": "profiles テーブルはUNIQUE(agent_id, user_id)制約でUPSERTする"},
    {"id": "m21", "content": "Zenn Bookのタイトルは『Claudeは明日もあなたを忘れる』に決定した"},
    {"id": "m22", "content": "CPU温度問題の原因はサーマルペースト/クーラー取り付け不良だった"},
    {"id": "m23", "content": "BSL 1.1ライセンスは教育・個人利用を許可している"},
    {"id": "m24", "content": "resolved=trueのエピソードはcompletion_factor 0.3で減衰する"},
    {"id": "m25", "content": "namespace形式はcpersona:{agent_id}でembedding serverに送信される"},
    {"id": "m26", "content": "WALモードはPRAGMA journal_mode=WALで有効化し並行読み取りを許可する"},
    {"id": "m27", "content": "COSINE_FLOORは0.20、COSINE_CEILは0.75がデフォルト値である"},
    {"id": "m28", "content": "purge_namespaceはnamespace内の全ベクトルを一括削除する"},
    {"id": "m29", "content": "msg_idによるべき等性でstore時の二重保存を防止する"},
    {"id": "m30", "content": "export_memoriesはJSONL形式で_typeフィールド付きのレコードを出力する"},
]

EPISODES = [
    {
        "summary": "CPersona v2.3.4のスケーラビリティ改善を実装。FTS5 memoriesインデックス追加、heapq top-K選択、adaptive scan limitを導入。",
        "keywords": "cpersona v2.3.4 scalability fts5 heapq scan-limit",
        "resolved": True,
    },
    {
        "summary": "embedding server v0.2.0にVectorIndex機能を追加。/index /search /remove /purgeエンドポイントとnamespace分離を実装。",
        "keywords": "embedding vector-index namespace search purge v0.2.0",
        "resolved": True,
    },
    {
        "summary": "Zenn Book全13章の執筆を完了。タイトルは『Claudeは明日もあなたを忘れる — MCP Memory Server cpersona 設計と実践』。",
        "keywords": "zenn-book writing cpersona mcp-memory chapters publishing",
        "resolved": False,
    },
]

# Queries with expected memory IDs (1-indexed matching MEMORIES list)
QUERIES = [
    ("プログラミング言語", ["m01"]),
    ("Tauri デスクトップ", ["m02"]),
    ("SQLite 全文検索", ["m03", "m14", "m16"]),
    ("MCP プロトコル 公開", ["m04"]),
    ("Discord bot", ["m05"]),
    ("Confidence Score 計算", ["m06", "m08"]),
    ("embedding server バージョン", ["m07"]),
    ("減衰率 半減期", ["m08"]),
    ("ベクトル次元 ONNX", ["m09"]),
    ("検索ステージ 多段", ["m10"]),
    ("agent_id 分離 セキュリティ", ["m11"]),
    ("エピソード要約 pre-computed", ["m12"]),
    ("プロファイル マージ LLM", ["m13"]),
    ("heapq 効率 ソート", ["m15"]),
    ("remote search 委譲", ["m17"]),
    ("BLOB サイズ 削減", ["m18"]),
    ("タスクキュー 永続化", ["m19"]),
    ("UPSERT UNIQUE 制約", ["m20"]),
    ("Zenn Book タイトル", ["m21"]),
    ("温度 サーマルペースト", ["m22"]),
    ("ライセンス 教育", ["m23"]),
    ("resolved 減衰 completion", ["m24"]),
    ("namespace embedding server", ["m25"]),
    ("WAL 並行 読み取り", ["m26"]),
    ("コサイン FLOOR CEIL", ["m27"]),
    ("purge 一括削除", ["m28"]),
    ("べき等 msg_id 重複", ["m29"]),
    ("JSONL export 形式", ["m30"]),
    ("v2.3.4 スケーラビリティ", ["m16"]),  # episode should also match
    ("chapters 執筆 publishing", []),  # episode-only query
]


def run_benchmark():
    client = httpx.Client(timeout=120)

    # 0. Clean up previous test data
    print("=== Cleanup ===")
    result = call_tool(client, "delete_agent_data", {"agent_id": AGENT_ID})
    print(f"  delete_agent_data: {result.get('data', result)}")

    # 1. Store memories
    print(f"\n=== Storing {len(MEMORIES)} memories ===")
    for mem in MEMORIES:
        result = call_tool(
            client,
            "store",
            {
                "agent_id": AGENT_ID,
                "message": {
                    "id": mem["id"],
                    "content": mem["content"],
                    "source": {"type": "System"},
                    "timestamp": "2026-03-28T10:00:00Z",
                },
            },
        )
        status = result.get("data", {})
        if status.get("skipped"):
            print(f"  {mem['id']}: skipped ({status.get('reason')})")
        else:
            print(f"  {mem['id']}: stored")

    # 2. Archive episodes
    print(f"\n=== Archiving {len(EPISODES)} episodes ===")
    for ep in EPISODES:
        result = call_tool(
            client,
            "archive_episode",
            {
                "agent_id": AGENT_ID,
                "history": [],
                "summary": ep["summary"],
                "keywords": ep["keywords"],
                "resolved": ep["resolved"],
            },
        )
        data = result.get("data", {})
        # MCP wraps in content[0].text
        if isinstance(data, dict) and "content" in data:
            text = data["content"][0]["text"] if data["content"] else "{}"
            inner = json.loads(text)
            print(f"  episode: {inner.get('episode_id', 'error')}")
        else:
            print(f"  episode: {data}")

    # Wait for indexing
    time.sleep(2)

    # 3. Run recall queries
    print(f"\n=== Running {len(QUERIES)} queries ===")
    results_at_5 = 0
    results_at_10 = 0
    reciprocal_ranks = []

    for query, expected_ids in QUERIES:
        result = call_tool(
            client,
            "recall",
            {"agent_id": AGENT_ID, "query": query, "limit": 10},
        )
        data = result.get("data", {})
        # MCP wraps in content[0].text
        if isinstance(data, dict) and "content" in data:
            text = data["content"][0]["text"] if data["content"] else "{}"
            inner = json.loads(text)
            messages = inner.get("messages", [])
        else:
            messages = data.get("messages", [])

        # Extract msg_ids from recalled content
        recalled_contents = [m.get("content", "") for m in messages]

        # Check if any expected memory content appears in results
        hit_at = None
        for rank, content in enumerate(recalled_contents):
            for eid in expected_ids:
                idx = int(eid[1:]) - 1
                if idx < len(MEMORIES) and MEMORIES[idx]["content"] in content:
                    if hit_at is None:
                        hit_at = rank + 1
                    break

        # Also check episode matches
        if not expected_ids:
            # Episode-only query — check if any [Episode] result exists
            for rank, content in enumerate(recalled_contents):
                if "[Episode]" in content:
                    hit_at = rank + 1
                    break

        hit_5 = hit_at is not None and hit_at <= 5
        hit_10 = hit_at is not None and hit_at <= 10

        if hit_5:
            results_at_5 += 1
        if hit_10:
            results_at_10 += 1
        if hit_at:
            reciprocal_ranks.append(1.0 / hit_at)
        else:
            reciprocal_ranks.append(0.0)

        status = f"rank={hit_at}" if hit_at else "MISS"
        print(f"  [{status:>8}] {query}")

    # 4. Calculate metrics
    total = len(QUERIES)
    recall_5 = results_at_5 / total
    recall_10 = results_at_10 / total
    mrr = sum(reciprocal_ranks) / total

    print(f"\n=== Benchmark Results ===")
    print(f"  Queries:    {total}")
    print(f"  Memories:   {len(MEMORIES)}")
    print(f"  Episodes:   {len(EPISODES)}")
    print(f"  Recall@5:   {recall_5:.1%} ({results_at_5}/{total})")
    print(f"  Recall@10:  {recall_10:.1%} ({results_at_10}/{total})")
    print(f"  MRR:        {mrr:.4f}")

    # 5. Cleanup
    print(f"\n=== Cleanup ===")
    result = call_tool(client, "delete_agent_data", {"agent_id": AGENT_ID})
    print(f"  delete_agent_data: {result.get('data', result)}")

    client.close()


if __name__ == "__main__":
    run_benchmark()
