#!/usr/bin/env python3
"""CPersona AB Test Runner — v2.4.14 current-state benchmark.

Auto-approves HITL command gates so the agent responds without manual intervention.
"""
import argparse, json, re, time, uuid, threading, queue
import urllib.request, urllib.parse, http.client

API_BASE = "http://127.0.0.1:8081/api"
API_KEY  = "d6b705613200449d6c9e08ecf218b0571742937c9575c26982c5be29b10443f3"
HEADERS  = {"Content-Type": "application/json", "X-API-Key": API_KEY}

QUERIES = [
    # Core drift test: asking about パン should NOT trigger ラズベリーパイ elaboration
    ("drift_trigger", "この前のパンの話覚えてる?"),
    ("drift_trigger", "昨日話したパンの件"),
    # Reverse: asking about ラズベリーパイ should NOT trigger パン elaboration
    ("reverse",       "ラズベリーパイについて覚えてる?"),
    ("reverse",       "Raspberry Pi って何?"),
    # Single keywords
    ("keyword",       "パン"),
    ("keyword",       "朝食"),
    ("keyword",       "ラズベリーパイ"),
    # Meta: "what did we discuss" — should summarize without injecting off-topic content
    ("meta",          "昨日何話してたっけ"),
    ("meta",          "このセッションで何話した?"),
    ("meta",          "私の好きな食べ物は?"),
    # Specific topic: only relevant if corpus contains it
    ("specific",      "git push の件"),
    ("specific",      "Discord の話"),
    # False positive: corpus contains NO memories on these topics
    # v2 — removed "週末の予定" (overlapped with "週末にパンを焼く" bread memory)
    ("false_pos",     "今日の天気"),
    ("false_pos",     "筋トレの話"),  # replaced: no corpus overlap
]

RASPI_KEYWORDS = ["ラズベリーパイ", "raspberry pi", "raspberrypi", "ラズパイ", "🥧"]
PAN_KEYWORDS   = ["パン", "bread", "ベーカリー", "bakery", "焼き立て"]
ELABORATION    = [r"[！!✨🥧🍞]", r"[？?]\s*$", r"(どこ|いつ|どんな|教えて|詳しく)",
                  r"(おいし|美味し|カリカリ|ふわふわ)"]

# Explicit no-memory disclaimers — agent is transparently saying "I don't have that"
# followed by listing available memories. This is COHERENT behavior, not drift.
DISMISSAL_PATTERNS = [
    r"(記憶がない|見つかりません|ありません|マッチしない|見つからない|見つけられない|なさそう|残っていない|存在しない)",
    r"(直接マッチ|関連.*ない|ない.*ですが|ないが|なく|該当.*なし)",
    r"(no.{0,20}(memory|record|match)|not found|don.t have|couldn.t find|no relevant)",
]


def classify(query: str, response: str) -> str:
    r = response.lower()
    is_pan   = any(k in query for k in PAN_KEYWORDS)
    is_raspi = any(k.lower() in query.lower() for k in RASPI_KEYWORDS)
    contam   = RASPI_KEYWORDS if is_pan else (PAN_KEYWORDS if is_raspi else RASPI_KEYWORDS + PAN_KEYWORDS)
    found    = any(k.lower() in r for k in contam)
    if not found:
        return "COHERENT"

    # Agent explicitly disclaimed having the memory → not genuine drift
    has_dismissal = any(re.search(p, response, re.IGNORECASE) for p in DISMISSAL_PATTERNS)
    if has_dismissal:
        # Still flag as SEVERE if the agent is actively elaborating on the unrelated topic
        # despite the disclaimer (rare, but possible)
        elab = sum(1 for p in ELABORATION if re.search(p, response))
        return "SEVERE" if elab >= 3 else "MILD"

    elab   = sum(1 for p in ELABORATION if re.search(p, response))
    kcount = sum(response.lower().count(k.lower()) for k in contam)
    if elab >= 2 or kcount >= 3:
        return "SEVERE"
    return "MILD"


def _post(path: str, body=None):
    payload = json.dumps(body or {}).encode()
    req = urllib.request.Request(
        f"{API_BASE}{path}", data=payload, headers=HEADERS, method="POST")
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read())


def send_message(agent_id: str, text: str) -> str:
    msg_id = str(uuid.uuid4())
    from datetime import datetime, timezone
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
        self.agent_id    = agent_id
        self.auto_approve = auto_approve
        self.q: queue.Queue = queue.Queue()
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _approve(self, approval_id: str):
        try:
            _post(f"/commands/{approval_id}/approve")
            print(f"  [auto-approve] {approval_id[:8]}…", flush=True)
        except Exception as e:
            print(f"  [auto-approve failed] {e}", flush=True)

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
                    block = buf.decode("utf-8", errors="replace")
                    buf = b""
                    data_str = None
                    for line in block.splitlines():
                        if line.startswith("data:"):
                            data_str = line[5:].strip()
                    if not data_str:
                        continue
                    try:
                        ev = json.loads(data_str)
                        etype = ev.get("type", "")
                        d     = ev.get("data", {})

                        if etype == "ThoughtResponse" and d.get("agent_id") == self.agent_id:
                            self.q.put(d.get("content", ""))

                        elif etype == "CommandApprovalRequested" and self.auto_approve:
                            if d.get("agent_id") == self.agent_id:
                                threading.Thread(
                                    target=self._approve,
                                    args=(d["approval_id"],),
                                    daemon=True,
                                ).start()
                    except Exception:
                        pass
        except Exception:
            pass
        finally:
            conn.close()

    def wait(self, timeout: float = 60.0) -> tuple[str, float]:
        t0 = time.monotonic()
        try:
            content = self.q.get(timeout=timeout)
            return content, time.monotonic() - t0
        except queue.Empty:
            return "", time.monotonic() - t0

    def stop(self):
        self._stop.set()


def run_benchmark(agent_id: str, trials: int):
    results = []
    total   = len(QUERIES) * trials

    print(f"\n{'='*60}")
    print(f"CPersona Benchmark — {agent_id}  (trials={trials})")
    print(f"Auto-approve: ON  (HITL gates bypass for benchmark)")
    print(f"{'='*60}\n")

    listener = SSEListener(agent_id, auto_approve=True)
    time.sleep(1.5)

    done = 0
    for category, query in QUERIES:
        for t in range(trials):
            done += 1
            print(f"[{done:3d}/{total}] {category:12s} | trial {t+1} | {query[:28]}", end="  ", flush=True)
            try:
                send_message(agent_id, query)
                content, latency = listener.wait(timeout=90.0)
                if not content:
                    verdict, sym = "TIMEOUT", "⏱"
                else:
                    verdict = classify(query, content)
                    sym = {"COHERENT": "✅", "MILD": "⚠️ ", "SEVERE": "❌"}.get(verdict, "?")
                print(f"{sym} {verdict} ({latency:.1f}s)")
                results.append({"query": query, "category": category, "trial": t+1,
                                 "verdict": verdict, "latency": latency,
                                 "content": content[:300]})
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
    errors    = [r for r in results   if r["verdict"] == "ERROR"]
    sev_rate  = len(severe)  / len(completed) * 100 if completed else 0
    avg_lat   = sum(r["latency"] for r in completed) / len(completed) if completed else 0

    print(f"\n{'='*60}")
    print(f"RESULTS  (v2.4.14 — per-agent threshold + XML fence)")
    print(f"{'='*60}")
    print(f"Total trials : {len(results)}  ({len(completed)} completed)")
    if completed:
        print(f"COHERENT     : {len(coherent):3d}  ({len(coherent)/len(completed)*100:.1f}%)")
        print(f"MILD         : {len(mild):3d}  ({len(mild)/len(completed)*100:.1f}%)")
        print(f"SEVERE drift : {len(severe):3d}  ({sev_rate:.1f}%)  ← key metric")
    print(f"TIMEOUT      : {len(timeouts):3d}")
    print(f"ERROR        : {len(errors):3d}")
    print(f"Latency avg  : {avg_lat:.1f}s")
    print(f"\nComparison (AB report, same query set, agent.cloto_default):")
    print(f"  A-v12  v2.4.12 baseline          : 23.1% severe")
    print(f"  C-xml  v2.4.13 AUTOCUT+XML fence  : 7.1%  severe")
    print(f"  Current v2.4.14 per-agent thresh  : {sev_rate:.1f}%  ← this run")
    print(f"{'='*60}\n")

    out = "/tmp/cpersona_benchmark_results.json"
    with open(out, "w") as f:
        json.dump({"agent_id": agent_id, "trials": trials, "results": results,
                   "summary": {"severe_rate": sev_rate, "avg_latency": avg_lat,
                               "n_completed": len(completed), "n_severe": len(severe),
                               "n_mild": len(mild), "n_coherent": len(coherent),
                               "n_timeout": len(timeouts)}}, f, ensure_ascii=False, indent=2)
    print(f"Full results → {out}")


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--trials", type=int, default=3)
    p.add_argument("--agent", default="agent.cloto_default")
    args = p.parse_args()
    run_benchmark(args.agent, args.trials)
