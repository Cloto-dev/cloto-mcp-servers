#!/usr/bin/env python3
"""
CPersona benchmark setup script.

Creates the dedicated benchmark agent (agent.cpersona_bench) in ClotoCore,
then loads a 100-item realistic diverse corpus into the CPersona DB with embeddings.

Usage:
    python3 cpersona_bench_setup.py [--reset] [--agent AGENT_ID]

Options:
    --reset     Delete all existing memories/episodes and reload corpus from scratch
    --agent     Agent ID to use (default: agent.cpersona_bench)

Requirements:
    - ClotoCore running on port 8081
    - Embedding server running on port 8401
"""

import argparse
import json
import sqlite3
import struct
import sys
import urllib.request
from datetime import datetime, timezone

# ── Configuration ──────────────────────────────────────────────────────────────

CLOTOCORE_API  = "http://127.0.0.1:8081/api"
EMBED_URL      = "http://127.0.0.1:8401/embed"
CLOTOCORE_DB   = "/Users/hachiya/Desktop/repos/ClotoCore/target/debug/data/cloto_memories.db"
CPERSONA_DB    = "/Users/hachiya/Desktop/repos/ClotoCore/dashboard/src-tauri/cpersona.db"
API_KEY        = "d6b705613200449d6c9e08ecf218b0571742937c9575c26982c5be29b10443f3"
DEFAULT_ENGINE = "mind.deepseek"
BENCH_AGENT_ID = "agent.cpersona_bench"

# ── 100-item realistic diverse corpus ─────────────────────────────────────────
# Messy, realistic, overlapping topics — close to real user memory distribution.
# パン and ラズベリーパイ are embedded naturally within food/tech categories.
# Format: (channel/topic, memory_text)

CORPUS: list[tuple[str, str]] = [
    # ── 食べ物・日常 (15件) ──
    ("food", "近所のパン屋で買ったクロワッサンがバターたっぷりで絶品だった"),
    ("food", "人生で初めてラズベリーパイ（デザート）を食べた。甘酸っぱくて美味しかった"),
    ("food", "朝ごはんに目玉焼きとトーストを食べた。シンプルだけど好き"),
    ("food", "ラーメン屋に並んで1時間待ったが、スープが絶品だった"),
    ("food", "スーパーでラズベリージャムを買った。ヨーグルトに入れると合う"),
    ("food", "週末に自分でパンを焼いた。生地をこねるのが思ったより難しかった"),
    ("food", "近所のカフェのチーズケーキが最高で、週一で通っている"),
    ("food", "夕食に鍋をした。白菜と豆腐がやさしい味だった"),
    ("food", "コンビニのおにぎりにハマっている。ツナマヨが特にお気に入り"),
    ("food", "友人と焼肉に行った。タン塩が一番好きだと改めて確認した"),
    ("food", "お気に入りのパン屋が閉店してしまって残念だった"),
    ("food", "スイーツ屋でラズベリータルトを頼んだ。見た目も可愛かった"),
    ("food", "朝食にアボカドトーストを試してみた。意外と美味しかった"),
    ("food", "ピザを手作りした。生地から作ると達成感がある"),
    ("food", "旬の苺が出てきたのでショートケーキを作った"),

    # ── 仕事・プロジェクト (12件) ──
    ("work", "プルリクエストのレビューが通ってようやくマージできた"),
    ("work", "会議が予定より1時間も延びてしまった。もう少し効率化したい"),
    ("work", "デプロイが失敗して夜中まで原因を調査した"),
    ("work", "TypeScriptの型エラーを直すのに半日かかった"),
    ("work", "同僚と新機能の設計について議論した。良い方向性がまとまった"),
    ("work", "月次レポートの締め切りが迫ってきて焦っている"),
    ("work", "新しいタスク管理ツールを導入した。チームの効率が上がりそう"),
    ("work", "コードレビューで丁寧なフィードバックをもらえて勉強になった"),
    ("work", "リモートワークの環境を整えた。モニターを増やしたら快適になった"),
    ("work", "スプリント振り返りでチームの課題が明確になった"),
    ("work", "オンボーディングで新人メンバーのサポートをした"),
    ("work", "技術的負債の解消タスクをコツコツ進めている"),

    # ── 旅行 (12件) ──
    ("travel", "来月の京都旅行の計画を立てた。嵐山と金閣寺は外せない"),
    ("travel", "新幹線の切符を早割で予約した。少し安く済んだ"),
    ("travel", "沖縄のホテルからの海の景色が最高だった"),
    ("travel", "海外旅行でパスポートの期限切れに気づいて焦った"),
    ("travel", "観光スポットをGoogleマップにまとめておいた"),
    ("travel", "旅行先で地元の朝市に行った。新鮮な野菜が安かった"),
    ("travel", "飛行機の遅延で乗り換えがギリギリになってひやひやした"),
    ("travel", "温泉旅館に泊まった。露天風呂が気持ちよかった"),
    ("travel", "旅行の荷物をできるだけ軽くするパッキング術を研究している"),
    ("travel", "外国語のメニューを読むのに苦労した"),
    ("travel", "旅先で偶然お祭りに遭遇した。運が良かった"),
    ("travel", "ホテルのチェックインがスムーズに終わって助かった"),

    # ── 健康・運動 (10件) ──
    ("health", "ジムに入会して3ヶ月。筋トレが習慣になってきた"),
    ("health", "健康診断の結果が返ってきた。コレステロールが少し高かった"),
    ("health", "ウォーキングを1時間続けたら体が軽くなった気がした"),
    ("health", "睡眠の質を改善するためにスマホを枕元に置くのをやめた"),
    ("health", "腰痛持ちなので、立ち仕事用のマットを買った"),
    ("health", "水分をもっと取るようにしたら肌の調子が良くなった"),
    ("health", "ストレッチを毎朝するようにしたら肩こりが減った"),
    ("health", "糖質制限を試してみたが、3日で挫折した"),
    ("health", "スポーツ用品店でランニングシューズを新調した"),
    ("health", "友人と一緒にヨガ教室に通い始めた"),

    # ── 趣味 (10件) ──
    ("hobby", "読書の習慣をつけようと決めて、月3冊を目標にしている"),
    ("hobby", "カメラを買ってから街歩きが楽しくなった"),
    ("hobby", "ギターを練習しているが、コードチェンジがまだぎこちない"),
    ("hobby", "映画館で話題の映画を観た。思ったより泣けた"),
    ("hobby", "将棋のアプリにハマっていて、通勤中によくやっている"),
    ("hobby", "DIYで棚を作った。水平を取るのが難しかった"),
    ("hobby", "アニメの新シリーズが始まって毎週楽しみにしている"),
    ("hobby", "家庭菜園でトマトを育てている。まだ小さい"),
    ("hobby", "編み物を始めた。マフラーを作ろうとしている"),
    ("hobby", "ボードゲームカフェに友人たちと行った。盛り上がった"),

    # ── 家族・友人 (10件) ──
    ("social", "親の誕生日に花束を送ったら喜んでくれた"),
    ("social", "幼なじみと10年ぶりに再会した。ほとんど変わっていなかった"),
    ("social", "子供が初めて自転車に乗れた。感動した"),
    ("social", "家族で近所の公園でバーベキューをした"),
    ("social", "友人の結婚式のスピーチを頼まれて緊張している"),
    ("social", "祖父母の家に久しぶりに帰省した"),
    ("social", "ペットの猫が体調を崩して動物病院に連れて行った"),
    ("social", "兄弟と久しぶりにオンラインゲームをした"),
    ("social", "友人のベビーシャワーのプレゼントを選ぶのが楽しかった"),
    ("social", "近所の人とゴミ出しのルールについて話し合った"),

    # ── 技術・PC (10件) ──
    ("tech", "Raspberry Pi 5 でホームサーバーを構築した。思ったより簡単だった"),
    ("tech", "MacBookのバッテリーが劣化してきたので交換を検討している"),
    ("tech", "Raspberry PiにDockerを入れてみた。動作が軽い"),
    ("tech", "3Dプリンターで部品を作った。設定が難しかった"),
    ("tech", "スマートホームの設定をした。照明をアプリで制御できるようになった"),
    ("tech", "古いPCをLinuxで復活させた。SSDに換装したら快適になった"),
    ("tech", "ラズパイでセンサーを使って温湿度をモニタリングしている"),
    ("tech", "VRヘッドセットを試した。少し酔いやすかった"),
    ("tech", "自作キーボードを組み立てた。はんだ付けが楽しかった"),
    ("tech", "ネットワークの設定を見直して、通信速度が改善した"),

    # ── 天気・季節 (8件) ──
    ("daily", "今日は急に寒くなったのでコートを引っ張り出した"),
    ("daily", "梅雨入りしたので傘を新調した"),
    ("daily", "台風が近づいているので外出を控えた"),
    ("daily", "今年の夏は特に暑くて、エアコンが手放せなかった"),
    ("daily", "桜が満開で、公園でお花見をした"),
    ("daily", "雪が積もったので子供と雪だるまを作った"),
    ("daily", "黄砂がひどくて洗濯物を外に干せなかった"),
    ("daily", "秋晴れが続いて気持ちいい。散歩日和だ"),

    # ── 買い物 (8件) ──
    ("shopping", "セールでジャケットを安く買えた"),
    ("shopping", "欲しかったヘッドフォンをやっと購入した"),
    ("shopping", "フリマアプリで不用品を売ったら思ったより高値がついた"),
    ("shopping", "電子レンジが壊れたので新しいものを選んでいる"),
    ("shopping", "本棚が足りなくなってきたので追加購入を検討している"),
    ("shopping", "ふるさと納税の返礼品が届いた。美味しかった"),
    ("shopping", "ポイントが貯まったので商品券と交換した"),
    ("shopping", "オンラインショッピングで誤注文してしまった"),

    # ── エンタメ・その他 (5件) ──
    ("entertainment", "ライブのチケットが当選した。久しぶりのコンサートで楽しみ"),
    ("entertainment", "Podcastを聴きながら通勤するのが最近の習慣"),
    ("entertainment", "地元の図書館で気になっていた本を予約した"),
    ("entertainment", "ニュースで見た社会問題について友人と議論した"),
    ("entertainment", "今年の目標を書き直した。去年達成できなかったものが多かった"),
]

# ── Helpers ────────────────────────────────────────────────────────────────────

def _embed_batch(texts: list[str], namespace: str) -> list[bytes]:
    data = json.dumps({"texts": texts, "namespace": namespace}).encode()
    req = urllib.request.Request(
        EMBED_URL, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        embeddings = json.loads(r.read())["embeddings"]
    return [struct.pack(f"{len(e)}f", *e) for e in embeddings]


def _check_services():
    print("Checking services...")
    try:
        req = urllib.request.Request(
            f"{CLOTOCORE_API}/system/health",
            headers={"X-API-Key": API_KEY})
        with urllib.request.urlopen(req, timeout=3) as r:
            r.read()
        print(f"  ClotoCore API: OK")
    except Exception as e:
        print(f"  ClotoCore API: FAILED — {e}")
        sys.exit(1)
    try:
        data = json.dumps({"texts": ["test"], "namespace": "bench"}).encode()
        req = urllib.request.Request(
            EMBED_URL, data=data, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as r:
            r.read()
        print(f"  Embedding server: OK")
    except Exception as e:
        print(f"  Embedding server: FAILED — {e}")
        sys.exit(1)


def _ensure_agent(agent_id: str):
    db = sqlite3.connect(CLOTOCORE_DB)
    try:
        row = db.execute("SELECT id FROM agents WHERE id=?", (agent_id,)).fetchone()
        if row:
            print(f"  Agent {agent_id}: already exists")
            return
        db.execute(
            "INSERT INTO agents (id, name, description, default_engine_id, status, "
            "metadata, required_capabilities, enabled, agent_type) VALUES (?,?,?,?,?,?,?,?,?)",
            (agent_id, "CPersona Benchmark",
             "Dedicated agent for CPersona recall quality benchmarks.",
             DEFAULT_ENGINE, "offline",
             json.dumps({"preferred_memory": "memory.cpersona"}),
             json.dumps([]), 1, "agent"))
        db.commit()
        print(f"  Agent {agent_id}: created")
    finally:
        db.close()


def _ensure_mcp_access(agent_id: str):
    db = sqlite3.connect(CLOTOCORE_DB)
    try:
        for server_id in ("memory.cpersona", "mind.deepseek"):
            row = db.execute(
                "SELECT id FROM mcp_access_control WHERE agent_id=? AND server_id=?",
                (agent_id, server_id)).fetchone()
            if not row:
                db.execute(
                    "INSERT INTO mcp_access_control "
                    "(agent_id, server_id, entry_type, permission, granted_at) VALUES (?,?,?,?,?)",
                    (agent_id, server_id, "server_grant", "allow",
                     datetime.now(timezone.utc).isoformat()))
        db.commit()
        print(f"  MCP access: OK")
    except Exception as e:
        print(f"  MCP access: skipped ({e})")
    finally:
        db.close()


def _reset_corpus(agent_id: str):
    db = sqlite3.connect(CPERSONA_DB)
    try:
        c1 = db.execute("DELETE FROM memories WHERE agent_id=?", (agent_id,))
        c2 = db.execute("DELETE FROM episodes WHERE agent_id=?", (agent_id,))
        db.commit()
        print(f"  Reset: deleted {c1.rowcount} memories, {c2.rowcount} episodes")
    finally:
        db.close()


def _load_corpus(agent_id: str):
    db = sqlite3.connect(CPERSONA_DB)
    total = 0
    now = datetime.now(timezone.utc).isoformat()
    namespace = f"cpersona:{agent_id}"
    try:
        batch_size = 10
        items = list(CORPUS)
        for i in range(0, len(items), batch_size):
            batch = items[i:i + batch_size]
            texts = [t for _, t in batch]
            channels = [c for c, _ in batch]
            n = i // batch_size + 1
            total_batches = (len(items) + batch_size - 1) // batch_size
            print(f"  Batch {n:2d}/{total_batches} ({len(texts)} items)...", end=" ", flush=True)
            blobs = _embed_batch(texts, namespace)
            for text, channel, blob in zip(texts, channels, blobs):
                db.execute(
                    "INSERT INTO memories "
                    "(agent_id, content, source, timestamp, embedding, channel, created_at, locked) "
                    "VALUES (?,?,?,?,?,?,?,?)",
                    (agent_id, text, json.dumps({"System": "benchmark_corpus"}),
                     now, blob, channel, now, 0))
                total += 1
            db.commit()
            print("done")
    finally:
        db.close()
    return total


def _verify_corpus(agent_id: str):
    db = sqlite3.connect(CPERSONA_DB)
    try:
        rows = db.execute(
            "SELECT channel, COUNT(*) FROM memories WHERE agent_id=? GROUP BY channel ORDER BY channel",
            (agent_id,)).fetchall()
        total = db.execute(
            "SELECT COUNT(*) FROM memories WHERE agent_id=?", (agent_id,)).fetchone()[0]
        with_emb = db.execute(
            "SELECT COUNT(*) FROM memories WHERE agent_id=? AND embedding IS NOT NULL",
            (agent_id,)).fetchone()[0]
        print(f"\n  Corpus ({agent_id}):")
        for channel, cnt in rows:
            print(f"    {channel:15s}: {cnt:3d}")
        print(f"    {'TOTAL':15s}: {total:3d}  (embeddings: {with_emb})")
    finally:
        db.close()


def main():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--reset", action="store_true")
    p.add_argument("--agent", default=BENCH_AGENT_ID)
    args = p.parse_args()

    print(f"\n{'='*55}")
    print(f"CPersona Benchmark Setup  ({len(CORPUS)} items)")
    print(f"Agent: {args.agent}")
    print(f"{'='*55}\n")

    _check_services()
    print()
    print("1. Setting up benchmark agent...")
    _ensure_agent(args.agent)
    _ensure_mcp_access(args.agent)
    print()
    print("2. Loading corpus...")
    if args.reset:
        _reset_corpus(args.agent)
    _load_corpus(args.agent)
    print()
    print("3. Verification...")
    _verify_corpus(args.agent)

    print(f"\n{'='*55}")
    print(f"Setup complete.")
    print(f"  python3 cpersona_ab_runner.py --agent {args.agent}")
    print(f"{'='*55}\n")


if __name__ == "__main__":
    main()
