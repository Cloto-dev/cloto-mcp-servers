#!/usr/bin/env python3
"""CPersona AB Test Runner — LLM Judge edition.

Uses DeepSeek API directly as an independent judge (case A: direct API).
Requires DEEPSEEK_API_KEY environment variable or CLOTOCORE_ENV_FILE path.

Auto-approves HITL command gates so the agent responds without manual intervention.

Usage:
    DEEPSEEK_API_KEY=sk-... python3 cpersona_ab_runner.py [--trials N] [--agent AGENT_ID]
    python3 cpersona_ab_runner.py --env /path/to/.env
"""
import argparse, json, os, re, time, uuid, threading, queue, http.client
import urllib.request, urllib.parse

API_BASE = "http://127.0.0.1:8081/api"
API_KEY  = "d6b705613200449d6c9e08ecf218b0571742937c9575c26982c5be29b10443f3"
HEADERS  = {"Content-Type": "application/json", "X-API-Key": API_KEY}

# DeepSeek judge config — override with CPERSONA_JUDGE_MODEL env var
DEEPSEEK_API_URL = "https://api.deepseek.com/v1/chat/completions"
JUDGE_MODEL      = os.environ.get("CPERSONA_JUDGE_MODEL", "deepseek-chat")

# ── Query set ─────────────────────────────────────────────────────────────────
# 14 queries covering realistic drift scenarios in a 100-item diverse corpus.

QUERIES = [
    # Core drift test: パンを聞いた時にラズベリーパイが出るか
    ("drift_trigger", "この前パン屋の話してたっけ？"),
    ("drift_trigger", "最近食べたもので印象に残ったものある？"),
    # Reverse: ラズベリーを聞いた時にパンが出るか
    ("reverse",       "ラズベリーのデザートについて覚えてる？"),
    ("reverse",       "ラズパイで何か作った話をしてたよね？"),
    # Single keyword — recall precision
    ("keyword",       "パン"),
    ("keyword",       "旅行"),
    ("keyword",       "健康"),
    # Meta — summarizes all memories; should not overweight food topics
    ("meta",          "最近何してた？"),
    ("meta",          "私の趣味は何だっけ？"),
    ("meta",          "最近食べたもの教えて"),
    # Specific topic present in corpus
    ("specific",      "gitの件"),
    ("specific",      "筋トレ始めた話"),
    # False positive — NOT in corpus at all
    ("false_pos",     "量子コンピュータについて"),
    ("false_pos",     "宇宙探査の最新ニュース"),
]

# ── LLM Judge ─────────────────────────────────────────────────────────────────

JUDGE_SYSTEM = "あなたは対話AIの応答品質を評価する専門の審査員です。指示に従い1単語で答えてください。"

JUDGE_PROMPT = """\
会話AIシステムの「トピックドリフト」を検出してください。

【定義】
トピックドリフト = クエリが求めていない別の話題をエージェントが詳述・展開すること。
同じ大カテゴリ（例：食べ物）でも、クエリが指定した具体的な話題（例：パン）と異なる具体的な話題（例：ラズベリーパイ）を主役にすると「ドリフト」です。

【クエリ】
{query}

【応答】
{response}

【判定基準】
- SEVERE: クエリが求めていない別の具体的な話題を中心に展開している（例: パンについて聞いたのにラズベリーパイを詳述）
- MILD: 別話題が少し登場するが脇役程度（「他にもこんな記憶があります」という言及のみ）
- COHERENT: クエリのトピックに沿って適切に応答している

COHERENT または MILD または SEVERE の1単語のみで答えてください。"""


def _load_deepseek_key(env_file=None) -> str:
    key = os.environ.get("DEEPSEEK_API_KEY", "")
    if key:
        return key
    candidates = [
        env_file,
        "/Users/hachiya/Desktop/repos/ClotoCore/.env",
        "/Users/hachiya/Desktop/repos/ClotoCore/dashboard/src-tauri/.env",
    ]
    for path in candidates:
        if path and os.path.exists(path):
            with open(path) as f:
                for line in f:
                    line = line.strip()
                    if line.startswith("DEEPSEEK_API_KEY="):
                        return line.split("=", 1)[1].strip()
    return ""


def judge_response(query: str, response: str, api_key: str) -> str:
    """Judge via DeepSeek API. Falls back to keyword heuristic on error."""
    if not api_key:
        return _keyword_fallback(query, response)
    payload = json.dumps({
        "model": JUDGE_MODEL,
        "messages": [
            {"role": "system", "content": JUDGE_SYSTEM},
            {"role": "user",   "content": JUDGE_PROMPT.format(
                query=query, response=response[:800])},
        ],
        "max_tokens": 10,
        "temperature": 0,
    }).encode()
    req = urllib.request.Request(
        DEEPSEEK_API_URL, data=payload,
        headers={"Content-Type": "application/json",
                 "Authorization": f"Bearer {api_key}"})
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            data = json.loads(r.read())
            text = data["choices"][0]["message"]["content"].strip().upper()
            for verdict in ("SEVERE", "MILD", "COHERENT"):
                if verdict in text:
                    return verdict
            return "COHERENT"
    except Exception as e:
        print(f"\n  [judge-api-error: {e}, fallback]", end="")
        return _keyword_fallback(query, response)


# ── Keyword fallback ──────────────────────────────────────────────────────────

_RASPI_KW = ["ラズベリーパイ", "raspberry pi", "raspberrypi", "ラズパイ"]
_PAN_KW   = ["パン", "bread", "ベーカリー", "bakery"]
_ELAB     = [r"[！!✨]", r"[？?]\s*$", r"(どこ|いつ|どんな|教えて|詳しく)",
             r"(おいし|美味し|カリカリ|ふわふわ)"]
_DISCLAIM = [r"(記憶がない|見つかりません|ありません|見つからない|なさそう)",
             r"(直接マッチ|関連.*ない|ないが|該当.*なし)"]


def _keyword_fallback(query: str, response: str) -> str:
    r = response.lower()
    is_pan   = any(k in query for k in _PAN_KW)
    is_raspi = any(k.lower() in query.lower() for k in _RASPI_KW)
    contam   = _RASPI_KW if is_pan else (_PAN_KW if is_raspi else _RASPI_KW + _PAN_KW)
    if not any(k.lower() in r for k in contam):
        return "COHERENT"
    if any(re.search(p, response, re.IGNORECASE) for p in _DISCLAIM):
        elab = sum(1 for p in _ELAB if re.search(p, response))
        return "SEVERE" if elab >= 3 else "MILD"
    elab   = sum(1 for p in _ELAB if re.search(p, response))
    kcount = sum(response.lower().count(k.lower()) for k in contam)
    return "SEVERE" if (elab >= 2 or kcount >= 3) else "MILD"


# ── ClotoCore API ─────────────────────────────────────────────────────────────

def _post(path: str, body=None):
    payload = json.dumps(body or {}).encode()
    req = urllib.request.Request(
        f"{API_BASE}{path}", data=payload, headers=HEADERS, method="POST")
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read())


def send_message(agent_id: str, text: str) -> str:
    from datetime import datetime, timezone
    msg_id = str(uuid.uuid4())
    _post("/chat", {
        "id": msg_id,
        "source": {"type": "User", "id": "benchmark", "name": "Benchmark"},
        "target_agent": agent_id,
        "content": text,
        "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "metadata": {"target_agent_id": agent_id},
    })
    return msg_id


class SSEListener:
    """Persistent SSE connection. Queues ThoughtResponse; auto-approves command gates."""

    def __init__(self, agent_id: str, auto_approve: bool = True):
        self.agent_id     = agent_id
        self.auto_approve = auto_approve
        self.q            = queue.Queue()
        self._stop        = threading.Event()
        self._thread      = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _approve(self, approval_id: str):
        try:
            _post(f"/commands/{approval_id}/approve")
            print(f"[approve {approval_id[:8]}] ", end="", flush=True)
        except Exception:
            pass

    def _run(self):
        conn = http.client.HTTPConnection("127.0.0.1", 8081)
        conn.request("GET", "/api/events",
                     headers={**HEADERS, "Accept": "text/event-stream",
                               "Cache-Control": "no-cache"})
        resp = conn.getresponse()
        buf = b""
        try:
            while not self._stop.is_set():
                chunk = resp.read(1)
                if not chunk:
                    break
                buf += chunk
                if buf.endswith(b"\n\n"):
                    block, buf = buf, b""
                    data_str = None
                    for line in block.decode("utf-8", errors="replace").splitlines():
                        if line.startswith("data:"):
                            data_str = line[5:].strip()
                    if not data_str:
                        continue
                    try:
                        ev = json.loads(data_str)
                        t  = ev.get("type", "")
                        d  = ev.get("data", {})
                        if t == "ThoughtResponse" and d.get("agent_id") == self.agent_id:
                            self.q.put(d.get("content", ""))
                        elif t == "CommandApprovalRequested" and self.auto_approve:
                            if d.get("agent_id") == self.agent_id:
                                threading.Thread(
                                    target=self._approve,
                                    args=(d["approval_id"],), daemon=True).start()
                    except Exception:
                        pass
        except Exception:
            pass
        finally:
            conn.close()

    def wait(self, timeout: float = 90.0):
        t0 = time.monotonic()
        try:
            content = self.q.get(timeout=timeout)
            return content, time.monotonic() - t0
        except queue.Empty:
            return "", time.monotonic() - t0

    def stop(self):
        self._stop.set()


# ── Main ──────────────────────────────────────────────────────────────────────

def run_benchmark(agent_id: str, trials: int, api_key: str):
    results = []
    total   = len(QUERIES) * trials
    judge_label = f"DeepSeek/{JUDGE_MODEL}" if api_key else "keyword-fallback"

    print(f"\n{'='*60}")
    print(f"CPersona Benchmark — {agent_id}")
    print(f"Trials : {trials}  |  Judge: {judge_label}")
    print(f"Corpus : 100 realistic items (9 topics)")
    print(f"{'='*60}\n")

    listener = SSEListener(agent_id, auto_approve=True)
    time.sleep(1.5)

    done = 0
    for category, query in QUERIES:
        for t in range(trials):
            done += 1
            print(f"[{done:3d}/{total}] {category:13s} | {t+1} | {query[:26]}",
                  end="  ", flush=True)
            try:
                send_message(agent_id, query)
                content, latency = listener.wait(timeout=90.0)
                if not content:
                    verdict, sym = "TIMEOUT", "⏱"
                else:
                    verdict = judge_response(query, content, api_key)
                    sym = {"COHERENT": "✅", "MILD": "⚠️ ", "SEVERE": "❌"}.get(verdict, "?")
                print(f"{sym} {verdict} ({latency:.1f}s)")
                results.append({"query": query, "category": category, "trial": t+1,
                                 "verdict": verdict, "latency": latency,
                                 "content": content[:400]})
                time.sleep(2.0)
            except Exception as e:
                print(f"ERROR: {e}")
                results.append({"query": query, "category": category, "trial": t+1,
                                 "verdict": "ERROR", "latency": 0, "content": ""})

    listener.stop()

    completed = [r for r in results if r["verdict"] not in ("TIMEOUT", "ERROR")]
    severe    = [r for r in completed if r["verdict"] == "SEVERE"]
    mild      = [r for r in completed if r["verdict"] == "MILD"]
    coherent  = [r for r in completed if r["verdict"] == "COHERENT"]
    timeouts  = [r for r in results   if r["verdict"] == "TIMEOUT"]
    sev_rate  = len(severe) / len(completed) * 100 if completed else 0
    avg_lat   = sum(r["latency"] for r in completed) / len(completed) if completed else 0

    print(f"\n{'='*60}")
    print(f"RESULTS  ({judge_label})")
    print(f"{'='*60}")
    print(f"Total   : {len(results)}  ({len(completed)} completed)")
    if completed:
        print(f"COHERENT: {len(coherent):3d}  ({len(coherent)/len(completed)*100:.1f}%)")
        print(f"MILD    : {len(mild):3d}  ({len(mild)/len(completed)*100:.1f}%)")
        print(f"SEVERE  : {len(severe):3d}  ({sev_rate:.1f}%)  ← key metric")
    print(f"TIMEOUT : {len(timeouts):3d}")
    print(f"Lat avg : {avg_lat:.1f}s")
    print(f"{'='*60}\n")

    out = "/tmp/cpersona_benchmark_results.json"
    with open(out, "w") as f:
        json.dump({"agent_id": agent_id, "trials": trials, "judge": judge_label,
                   "results": results,
                   "summary": {"severe_rate": sev_rate, "avg_latency": avg_lat,
                               "n_completed": len(completed), "n_severe": len(severe),
                               "n_mild": len(mild), "n_coherent": len(coherent),
                               "n_timeout": len(timeouts)}},
                  f, ensure_ascii=False, indent=2)
    print(f"Results → {out}")
    return results


def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--trials", type=int, default=3)
    p.add_argument("--agent",  default="agent.cpersona_bench")
    p.add_argument("--env",    default=None, help="Path to .env file")
    args = p.parse_args()

    api_key = _load_deepseek_key(args.env)
    if not api_key:
        print("WARNING: DEEPSEEK_API_KEY not found — using keyword fallback judge")

    run_benchmark(args.agent, args.trials, api_key)


if __name__ == "__main__":
    main()
