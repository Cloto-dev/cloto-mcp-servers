"""CPersona Pipeline Evaluation using LMEB data.

Evaluates CPersona's Cascading Recall pipeline against LoCoMo and LongMemEval
ground truth data from the LMEB benchmark.

Requires ClotoCore running at http://127.0.0.1:8081.

Usage:
  python scripts/benchmark_cpersona_lmeb.py --tasks LoCoMo
  python scripts/benchmark_cpersona_lmeb.py --tasks LoCoMo,LongMemEval
"""

import argparse
import csv
import json
import logging
import os
import sys
import time
from collections import defaultdict

import httpx

logging.basicConfig(
    format="%(levelname)s|%(asctime)s|%(name)s: %(message)s",
    datefmt="%Y/%m/%d %H:%M:%S",
    level=logging.INFO,
)
logger = logging.getLogger("benchmark_cpersona_lmeb")

API_BASE = "http://127.0.0.1:8081"
API_KEY = "d6b705613200449d6c9e08ecf218b0571742937c9575c26982c5be29b10443f3"
SERVER_ID = "memory.cpersona"
AGENT_ID = "lmeb-bench"
HEADERS = {"X-API-Key": API_KEY, "Content-Type": "application/json"}

LMEB_DIR = os.environ.get(
    "LMEB_DIR",
    os.path.join(os.environ.get("LOCALAPPDATA", ""), "Temp", "lmeb"),
)
EVAL_DATA = os.path.join(LMEB_DIR, "eval_data")

# Task definitions matching LMEB structure
TASK_DEFS = {
    "LoCoMo": {
        "data_path": "Dialogue/LoCoMo",
        "subtasks": ["single_hop", "multi_hop", "temporal_reasoning", "open_domain", "adversarial"],
        "corpus_shared": True,  # corpus.jsonl at task level, queries per subtask
    },
    "LongMemEval": {
        "data_path": "Dialogue/LongMemEval",
        "subtasks": [
            "knowledge_update", "multi_session",
            "single_session_assistant", "single_session_preference",
            "single_session_user", "temporal_reasoning",
        ],
        "corpus_shared": True,
    },
}


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


def load_corpus(corpus_path: str) -> dict[str, dict]:
    """Load corpus.jsonl → {id: {title, text}}"""
    corpus = {}
    with open(corpus_path, "r", encoding="utf-8") as f:
        for line in f:
            doc = json.loads(line)
            corpus[doc["id"]] = {
                "title": doc.get("title", ""),
                "text": doc.get("text", ""),
            }
    return corpus


def load_queries(queries_path: str) -> dict[str, str]:
    """Load queries.jsonl → {id: text}"""
    queries = {}
    with open(queries_path, "r", encoding="utf-8") as f:
        for line in f:
            q = json.loads(line)
            queries[q["id"]] = q["text"]
    return queries


def load_qrels(qrels_path: str) -> dict[str, dict[str, int]]:
    """Load qrels.tsv → {query_id: {doc_id: score}}"""
    qrels = {}
    with open(qrels_path, "r", encoding="utf-8") as f:
        reader = csv.DictReader(f, delimiter="\t", fieldnames=["query-id", "corpus-id", "score"])
        for row in reader:
            qid = row["query-id"]
            did = row["corpus-id"]
            score = int(row["score"])
            qrels.setdefault(qid, {})[did] = score
    return qrels


def load_candidates(candidates_path: str) -> dict[str, list[str]]:
    """Load candidates.jsonl → {scene_id: [doc_ids]}"""
    mapping = {}
    if not os.path.exists(candidates_path):
        return mapping
    with open(candidates_path, "r", encoding="utf-8") as f:
        for line in f:
            data = json.loads(line)
            mapping[str(data["scene_id"])] = data["candidate_doc_ids"]
    return mapping


def get_scene_id(query_id: str) -> str:
    """Extract scene_id from query_id (e.g., 'scene_1_q1' → 'scene_1')."""
    parts = query_id.split("_")
    if len(parts) >= 2:
        return "_".join(parts[:2])
    return query_id


def store_corpus(client: httpx.Client, corpus: dict[str, dict], batch_report: int = 100):
    """Store corpus documents as memories in CPersona."""
    total = len(corpus)
    stored = 0
    for i, (doc_id, doc) in enumerate(corpus.items(), 1):
        content = (doc.get("title", "") + " " + doc.get("text", "")).strip()
        if not content:
            continue
        call_tool(client, "store", {
            "agent_id": AGENT_ID,
            "message": {
                "id": doc_id,
                "content": content,
                "source": {"type": "System"},
                "timestamp": "2026-01-01T00:00:00Z",
            },
        })
        stored += 1
        if i % batch_report == 0:
            logger.info(f"  Stored {i}/{total} documents")

    logger.info(f"  Total stored: {stored}/{total}")
    return stored


def evaluate_recall(
    client: httpx.Client,
    queries: dict[str, str],
    qrels: dict[str, dict[str, int]],
    candidates: dict[str, list[str]],
    k_values: list[int] = [1, 5, 10, 25, 50],
    recall_limit: int = 50,
) -> dict[str, float]:
    """Run recall queries and compute retrieval metrics."""

    # Per-query results: {query_id: {doc_id: score}}
    results: dict[str, dict[str, float]] = {}

    total = len(queries)
    for i, (qid, query_text) in enumerate(queries.items(), 1):
        if i % 50 == 0:
            logger.info(f"  Queried {i}/{total}")

        result = call_tool(client, "recall", {
            "agent_id": AGENT_ID,
            "query": query_text,
            "limit": recall_limit,
        })

        # Parse MCP response
        data = result.get("data", {})
        if isinstance(data, dict) and "content" in data:
            text = data["content"][0]["text"] if data["content"] else "{}"
            inner = json.loads(text)
            messages = inner.get("messages", [])
        else:
            messages = data.get("messages", [])

        # Build score dict from recall results (use position-based scoring)
        doc_scores = {}
        for rank, msg in enumerate(messages):
            msg_id = msg.get("msg_id", "")
            if msg_id:
                doc_scores[msg_id] = 1.0 / (rank + 1)  # Reciprocal rank as score

        # Filter by candidates (same as LMEB SubsetRetrieval)
        if candidates:
            scene_id = get_scene_id(qid)
            if scene_id in candidates:
                allowed = set(candidates[scene_id])
                doc_scores = {k: v for k, v in doc_scores.items() if k in allowed}

        results[qid] = doc_scores

    # Compute metrics
    metrics = compute_metrics(qrels, results, k_values)
    return metrics


def compute_metrics(
    qrels: dict[str, dict[str, int]],
    results: dict[str, dict[str, float]],
    k_values: list[int],
) -> dict[str, float]:
    """Compute NDCG@k, Recall@k, R_cap@k, MRR."""
    import math

    ndcg_scores = defaultdict(list)
    recall_scores = defaultdict(list)
    rcap_scores = defaultdict(list)
    mrr_scores = []

    for qid in qrels:
        if qid not in results:
            continue

        doc_scores = results[qid]
        ranked = sorted(doc_scores.items(), key=lambda x: x[1], reverse=True)
        relevant = {d: s for d, s in qrels[qid].items() if s > 0}
        num_relevant = len(relevant)

        if num_relevant == 0:
            continue

        # MRR
        rr = 0.0
        for rank, (doc_id, _) in enumerate(ranked, 1):
            if doc_id in relevant:
                rr = 1.0 / rank
                break
        mrr_scores.append(rr)

        for k in k_values:
            top_k = ranked[:k]
            top_k_ids = [d for d, _ in top_k]

            # Recall@k
            hits = sum(1 for d in top_k_ids if d in relevant)
            recall_scores[k].append(hits / num_relevant)

            # R_cap@k
            denom = min(num_relevant, k)
            rcap_scores[k].append(hits / denom if denom > 0 else 0)

            # NDCG@k
            dcg = 0.0
            for i, doc_id in enumerate(top_k_ids):
                if doc_id in relevant:
                    dcg += 1.0 / math.log2(i + 2)
            ideal = sum(1.0 / math.log2(i + 2) for i in range(min(num_relevant, k)))
            ndcg_scores[k].append(dcg / ideal if ideal > 0 else 0)

    metrics = {}
    for k in k_values:
        if ndcg_scores[k]:
            metrics[f"ndcg_at_{k}"] = sum(ndcg_scores[k]) / len(ndcg_scores[k])
        if recall_scores[k]:
            metrics[f"recall_at_{k}"] = sum(recall_scores[k]) / len(recall_scores[k])
        if rcap_scores[k]:
            metrics[f"R_cap_at_{k}"] = sum(rcap_scores[k]) / len(rcap_scores[k])

    if mrr_scores:
        metrics["mrr"] = sum(mrr_scores) / len(mrr_scores)

    return metrics


def run_task(client: httpx.Client, task_name: str, output_dir: str):
    """Run CPersona pipeline evaluation for a single LMEB task."""
    task_def = TASK_DEFS[task_name]
    data_path = os.path.join(EVAL_DATA, task_def["data_path"])

    # Load shared corpus
    corpus_path = os.path.join(data_path, "corpus.jsonl")
    corpus = load_corpus(corpus_path)
    logger.info(f"Corpus: {len(corpus)} documents")

    # Load candidates
    candidates_path = os.path.join(data_path, "candidates.jsonl")
    candidates = load_candidates(candidates_path)
    logger.info(f"Candidates: {len(candidates)} scenes")

    # Clean previous data
    logger.info("Cleaning previous data...")
    call_tool(client, "delete_agent_data", {"agent_id": AGENT_ID})

    # Store corpus
    logger.info(f"Storing {len(corpus)} documents...")
    store_start = time.time()
    store_corpus(client, corpus)
    store_time = time.time() - store_start
    logger.info(f"Store completed in {store_time:.1f}s")

    # Wait for indexing
    logger.info("Waiting for embedding indexing...")
    time.sleep(5)

    # Evaluate each subtask
    task_results = {}
    for subtask in task_def["subtasks"]:
        logger.info(f"\n--- {task_name}/{subtask} ---")

        queries_path = os.path.join(data_path, subtask, "queries.jsonl")
        qrels_path = os.path.join(data_path, subtask, "qrels.tsv")

        if not os.path.exists(queries_path):
            logger.warning(f"  Missing queries: {queries_path}")
            continue

        queries = load_queries(queries_path)
        qrels = load_qrels(qrels_path)
        logger.info(f"  Queries: {len(queries)}, Qrels: {len(qrels)}")

        eval_start = time.time()
        metrics = evaluate_recall(client, queries, qrels, candidates)
        eval_time = time.time() - eval_start

        task_results[subtask] = metrics
        ndcg10 = metrics.get("ndcg_at_10", 0)
        r10 = metrics.get("recall_at_10", 0)
        logger.info(f"  NDCG@10={ndcg10*100:.2f}, R@10={r10*100:.2f} ({eval_time:.1f}s)")

    # Cleanup
    logger.info("Cleaning up...")
    call_tool(client, "delete_agent_data", {"agent_id": AGENT_ID})

    # Save results
    os.makedirs(output_dir, exist_ok=True)
    result_path = os.path.join(output_dir, f"{task_name}.json")
    output = {
        "task": task_name,
        "agent_id": AGENT_ID,
        "store_time_s": store_time,
        "corpus_size": len(corpus),
        "subtasks": task_results,
    }

    # Compute mean
    all_ndcg10 = [m.get("ndcg_at_10", 0) for m in task_results.values() if m]
    if all_ndcg10:
        output["mean_ndcg_at_10"] = sum(all_ndcg10) / len(all_ndcg10)

    with open(result_path, "w") as f:
        json.dump(output, f, indent=2)
    logger.info(f"Results saved to {result_path}")

    return output


def main():
    parser = argparse.ArgumentParser(description="CPersona Pipeline LMEB Evaluation")
    parser.add_argument(
        "--tasks",
        default="LoCoMo,LongMemEval",
        help="Comma-separated task names",
    )
    parser.add_argument(
        "--output_dir",
        default="cpersona_lmeb_results",
        help="Output directory",
    )
    args = parser.parse_args()

    task_names = [t.strip() for t in args.tasks.split(",")]

    client = httpx.Client(timeout=120)

    for task_name in task_names:
        if task_name not in TASK_DEFS:
            logger.error(f"Unknown task: {task_name}. Available: {list(TASK_DEFS.keys())}")
            continue

        logger.info(f"\n{'='*60}")
        logger.info(f"Task: {task_name}")
        logger.info(f"{'='*60}")

        result = run_task(client, task_name, args.output_dir)

        mean = result.get("mean_ndcg_at_10", 0)
        logger.info(f"\n{task_name} Mean NDCG@10: {mean*100:.2f}")

    client.close()


if __name__ == "__main__":
    main()
