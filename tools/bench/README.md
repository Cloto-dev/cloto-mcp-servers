# CPersona Benchmark Tools

Drift measurement harness for CPersona recall quality.

## Setup

Requires ClotoCore running on port 8081 and embedding server on port 8401.
Set `CLOTO_YOLO=true` in ClotoCore `.env` to avoid HITL approval delays.

```bash
# Create benchmark agent + load 100-item corpus (--reset wipes all memories/episodes)
python3 cpersona_bench_setup.py --reset

# Run single-arm benchmark (N=3, ~8 min)
DEEPSEEK_API_KEY=sk-... python3 cpersona_ab_runner.py --agent agent.cpersona_bench --trials 3

# Run 4-arm comparative benchmark (N=10, ~90 min)
DEEPSEEK_API_KEY=sk-... python3 cpersona_bench_all_arms.py --trials 10
```

## Benchmark agent

`agent.cpersona_bench` — dedicated ClotoCore agent. Do not use for production.

## Corpus (100 items, 9 topics)

Realistic diverse memories, close to real user distribution.
`パン` and `ラズベリーパイ` are embedded naturally within `food` and `tech` topics.

| Channel | Topic | Items |
|---|---|---|
| `food` | 食べ物・日常 (includes パン and ラズベリーパイ) | 15 |
| `work` | 仕事・プロジェクト | 12 |
| `travel` | 旅行 | 12 |
| `health` | 健康・運動 | 10 |
| `hobby` | 趣味 | 10 |
| `social` | 家族・友人 | 10 |
| `tech` | 技術・PC (includes Raspberry Pi) | 10 |
| `daily` | 天気・季節 | 8 |
| `shopping` | 買い物 | 8 |
| `entertainment` | エンタメ・その他 | 5 |

## Query set (14 queries × N trials)

| Category | Purpose |
|---|---|
| `drift_trigger` | パン → ラズベリーパイ cross-topic drift を検出 |
| `reverse` | ラズベリー/ラズパイ → パン 逆方向 drift を検出 |
| `keyword` | 単一キーワードで関連メモリを正しく召喚できるか |
| `meta` | 「最近何した?」型クエリで全メモリを適切に要約できるか |
| `specific` | コーパス内の特定トピック（git, 筋トレ）を正確に召喚できるか |
| `false_pos` | コーパスにないトピック（量子コンピュータ, 宇宙）で誤召喚しないか |

## LLM Judge

DeepSeek API を直接呼び出す独立した審査員（ClotoCore と別プロセス）。

- **モデル**: `deepseek-chat`（DeepSeek バックエンドで `deepseek-v4-flash` にルーティング）
- **コスト**: ~$0.001/trial
- **フォールバック**: API エラー時はキーワードヒューリスティックに自動切り替え
- **API KEY**: `DEEPSEEK_API_KEY` 環境変数、または ClotoCore `.env` から自動ロード

## Arm switcher

4 つのフィーチャーアームを env var 切り替えで計測。コードは全アームで同一（v2.4.15）。

```bash
python3 cpersona_arm_switch.py list     # アーム一覧
python3 cpersona_arm_switch.py v2413    # v2413 に切り替え（プロセス自動再起動）
```

| Arm | 特徴 |
|---|---|
| `v2412` | chat-turn 形式、AUTOCUT 無効、episode penalty 無効 |
| `v2413` | XML fence + AUTOCUT、episode penalty 無効 |
| `v2414` | + episode penalty（global threshold） |
| `v2415` | + `CPERSONA_AUTO_CALIBRATE=true`（per-agent threshold） |

## Results

### 4-arm N=10 comparative benchmark (2026-05-02)

**Judge**: DeepSeek/deepseek-v4-flash (direct API)
**Corpus**: 100 realistic items | **Trials**: 140/arm | **Total**: 560

| Arm | Severe% | drift_trigger | reverse | keyword | meta | specific | false_pos |
|---|---|---|---|---|---|---|---|
| v2412 | 10.7% | **0%** | **0%** | 37% | 7% | 10% | **0%** |
| v2413 | 10.0% | **0%** | **0%** | 37% | 10% | **0%** | **0%** |
| v2414 | 11.4% | **0%** | **0%** | 33% | 13% | 10% | **0%** |
| v2415 | 10.7% | **0%** | **0%** | 40% | 10% | **0%** | **0%** |

**Key findings:**
- `drift_trigger` / `reverse` / `false_pos`: **0% across all arms** — original パン↔ラズベリーパイ drift is fully resolved
- `keyword` 33–40% SEVERE: dominated by `健康` queries where food/health topics semantically overlap in corpus (e.g. 糖質制限 = both health and food)
- Arm differences (10.0–11.4%) are within noise range — XML fence / AUTOCUT / episode penalty effects are saturated on this corpus
- **The remaining SEVERE cases are semantic embedding-level overlap, not presentation-layer drift** — target for Cross-Encoder re-ranking

### Controlled 3-arm benchmark (2026-05-02, keyword rubric)

Earlier benchmark with 19-item controlled corpus and keyword-based classifier.
Same codebase, different corpus and judge.

| Arm | Severe% |
|---|---|
| v2412 | 2.4% |
| v2413 | **0.0%** |
| v2414 | 2.4% |

### Original AB report (2026-04-24, contaminated DB, keyword rubric)

Different conditions: `agent.cloto_default`, 108 mixed memories, v2.4.12 quality-gate bug present.

| Arm | Severe% |
|---|---|
| A-v12 (v2.4.12) | 23.1% |
| C-xml (v2.4.13) | 7.1% |

## Notes

- **YOLO mode** (`CLOTO_YOLO=true`): eliminates HITL approval delays; required for stable benchmarks
- **Arm isolation**: env vars only — all arms run v2.4.15 code; the code that connects `CPERSONA_AUTO_CALIBRATE` was added in v2.4.15 (was a no-op in v2.4.14)
- **`keyword 健康` contamination**: corpus item "糖質制限を試してみた" is semantically dual (health AND food) — a known false-positive source in the judge
- **Next step**: Cross-Encoder re-ranking to resolve semantic-level contamination (jina-v5-nano limitation)
